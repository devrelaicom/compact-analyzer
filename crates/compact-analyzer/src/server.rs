//! The LSP server: synchronous main loop over stdio.
//!
//! Never-die contract: every message dispatch runs under catch_unwind; a
//! panicked request gets an InternalError response and the loop continues.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use analyzer_core::{AnalysisHost, FileId, LineIndex, TextRange, TextSize};
use analyzer_toolchain::{CompileOutcome, CompileStatus, Toolchain};
use crossbeam_channel::RecvTimeoutError;
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, PublishDiagnosticsParams,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};

use crate::lsp_utils;

const DEBOUNCE: Duration = Duration::from_millis(150);

/// Wall-clock ceiling on a single compile-on-save invocation (OQ7). Also
/// bounds worst-case shutdown latency while a compile is literally in flight:
/// `run()` never joins worker threads, but `io_threads.join()` waits for every
/// `sender` clone to drop, and an in-flight worker holds one until
/// `compile_file` returns — which this timeout bounds. No config key.
const COMPILE_TIMEOUT: Duration = Duration::from_secs(30);

/// Diagnostics stored per open file. `HashMap` keyed by `FileId` under a
/// `Mutex`, wrapped in an `Arc` so a compile worker (which cannot touch the
/// single-threaded `host`) can store its results and the main thread's
/// `publish_file_diagnostics` can re-merge them.
type CompilerDiagnostics = Arc<Mutex<HashMap<FileId, Vec<lsp_types::Diagnostic>>>>;

/// Per-file monotonic save generation, used to drop stale compile results
/// (thread-per-save, superseded workers publish nothing).
type SaveGenerations = Arc<Mutex<HashMap<FileId, u64>>>;

/// Locks a `Mutex`, recovering the guard even if a previous holder panicked
/// (never-die: a poisoned lock must not wedge every later compile).
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// JSON-RPC error codes (avoid depending on lsp-server exporting them).
const METHOD_NOT_FOUND: i32 = -32601;
const INTERNAL_ERROR: i32 = -32603;
const REQUEST_FAILED: i32 = -32803;

pub(crate) fn run() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();

    let (initialize_id, initialize_params) = connection.initialize_start()?;
    let capabilities = ServerCapabilities {
        // Options form (was `Kind(FULL)`): keeps FULL-document change sync and
        // additionally advertises `save`, so the client sends `didSave` — the
        // trigger for compile-on-save. `includeText` is intentionally omitted:
        // the compiler reads the saved file from disk, not from the payload.
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            lsp_types::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(lsp_types::TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        definition_provider: Some(lsp_types::OneOf::Left(true)),
        references_provider: Some(lsp_types::OneOf::Left(true)),
        rename_provider: Some(lsp_types::OneOf::Left(true)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(lsp_types::OneOf::Left(true)),
        workspace_symbol_provider: Some(lsp_types::OneOf::Left(true)),
        folding_range_provider: Some(lsp_types::FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(lsp_types::SelectionRangeProviderCapability::Simple(true)),
        completion_provider: Some(lsp_types::CompletionOptions {
            trigger_characters: Some(vec![".".to_string()]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
        semantic_tokens_provider: Some(
            lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                lsp_types::SemanticTokensOptions {
                    work_done_progress_options: Default::default(),
                    legend: crate::semantic_tokens_legend::legend(),
                    range: Some(false),
                    full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                },
            ),
        ),
        ..Default::default()
    };
    let initialize_result = serde_json::json!({
        "capabilities": capabilities,
        "serverInfo": {
            "name": "compact-analyzer",
            "version": env!("CARGO_PKG_VERSION"),
        },
    });
    connection.initialize_finish(initialize_id, initialize_result)?;
    eprintln!("compact-analyzer: initialized");

    let mut state = GlobalState::new(connection.sender.clone(), connection.receiver.clone());
    let stdlib_dir = std::env::temp_dir().join(format!(
        "compact-analyzer-stdlib-{}",
        env!("CARGO_PKG_VERSION")
    ));
    match analyzer_core::stdlib::materialize(&stdlib_dir) {
        Ok(path) => {
            state.host.register_stdlib(&path);
            eprintln!("compact-analyzer: stdlib stub at {}", path.display());
        }
        Err(err) => {
            eprintln!("compact-analyzer: stdlib unavailable ({err}); continuing without it")
        }
    }

    // M2b: consume workspace configuration and build the eager index.
    state.host.set_import_search_path(import_search_path_from(
        initialize_params.get("initializationOptions"),
    ));
    state.workspace_roots = workspace_roots_from_params(&initialize_params);
    state.watch_supported = client_supports_watch(&initialize_params);
    state.toolchain_config = toolchain_config_from(initialize_params.get("initializationOptions"));
    // Discover the optional `compact` toolchain once, honoring an explicit
    // override path. Absence is a first-class state: with no toolchain,
    // compile-on-save is simply never attempted (a clean no-op, no error spam).
    state.toolchain = Toolchain::discover(state.toolchain_config.toolchain_path.as_deref());
    match &state.toolchain {
        Some(tc) => eprintln!(
            "compact-analyzer: compact toolchain {} (language {}) at {}",
            tc.tool_version,
            tc.language_version,
            tc.compact_bin.display()
        ),
        None => eprintln!("compact-analyzer: no compact toolchain found; compile-on-save disabled"),
    }
    eprintln!(
        "compact-analyzer: indexing {} workspace root(s)",
        state.workspace_roots.len()
    );
    // Never-die: index each discovered file under its own catch_unwind so a
    // single pathological `.compact` file can't unwind through `run()` and
    // crash the process before the main loop even starts (this crawl runs
    // before the client sees anything but `initialize`). One bad file is
    // logged and skipped; the rest of the workspace still gets indexed.
    let roots = state.workspace_roots.clone();
    for path in analyzer_core::discover_compact_files(&roots) {
        let file = state.host.vfs_mut().file_id(&path);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.host.index_file(file)));
        if let Err(panic) = result {
            eprintln!(
                "compact-analyzer: panic while indexing {}: {}",
                path.display(),
                panic_message(panic.as_ref())
            );
        }
    }
    if state.watch_supported {
        state.register_file_watcher();
    }

    loop {
        // When diagnostics are pending, wait only until the debounce
        // deadline; otherwise block until the next message.
        let next_deadline = state.pending_diagnostics.values().min().copied();
        let msg = if let Some(deadline) = next_deadline {
            let now = Instant::now();
            if deadline <= now {
                state.flush_pending_diagnostics();
                continue;
            }
            match connection.receiver.recv_timeout(deadline - now) {
                Ok(msg) => msg,
                Err(RecvTimeoutError::Timeout) => {
                    state.flush_pending_diagnostics();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match connection.receiver.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            }
        };

        if let Message::Request(req) = &msg
            && connection.handle_shutdown(req)?
        {
            break;
        }
        state.dispatch(msg);
    }

    // `io_threads`'s writer thread only exits once every clone of
    // `connection.sender` is dropped (its receiving iterator ends when the
    // channel closes). `state` holds a clone, `connection` holds the original,
    // and each in-flight compile worker holds one too. Signal shutdown first so
    // workers skip publishing and drop their sender promptly once their compile
    // returns; then drop our two clones. `io_threads.join()` then blocks only
    // until the last in-flight worker's `compile_file` returns — bounded by
    // `COMPILE_TIMEOUT`, so shutdown is never-hang (see that constant's docs).
    state.shutdown.store(true, Ordering::SeqCst);
    drop(state);
    drop(connection);
    io_threads.join()?;
    eprintln!("compact-analyzer: shut down");
    Ok(())
}

struct GlobalState {
    sender: crossbeam_channel::Sender<Message>,
    host: AnalysisHost,
    /// Documents currently open in the editor, by URI.
    open_files: HashMap<Url, FileId>,
    /// Files with not-yet-published diagnostics, each with its own deadline.
    pending_diagnostics: HashMap<FileId, Instant>,
    /// Roots to (re)crawl for `.compact` files.
    workspace_roots: Vec<PathBuf>,
    /// The client supports dynamic `didChangeWatchedFiles` registration.
    watch_supported: bool,
    /// v1 toolchain configuration surface (design §4): toolchain path,
    /// compile-on-save, and formatting toggles from `initializationOptions`.
    toolchain_config: ToolchainConfig,
    /// The discovered `compact` toolchain, or `None` when absent (compile-on-save
    /// is then a no-op). Cloned into each compile worker.
    toolchain: Option<Toolchain>,
    /// Per-file compiler diagnostics, shared with compile workers. Merged into
    /// the native set by both a worker (on save) and `publish_file_diagnostics`
    /// (so a debounced native republish doesn't wipe them).
    compiler_diagnostics: CompilerDiagnostics,
    /// Per-file save generation; a worker whose captured generation is no longer
    /// current drops its result (supersession).
    save_generations: SaveGenerations,
    /// Set true at teardown so in-flight workers skip publishing and drop their
    /// `sender` clone promptly (bounded, never-hang shutdown).
    shutdown: Arc<AtomicBool>,
    /// A clone of the connection receiver, used only to test emptiness for
    /// cooperative cancellation (single-threaded — no concurrent consumer).
    cancel_receiver: crossbeam_channel::Receiver<Message>,
    /// Id counter for server→client requests (e.g. registerCapability).
    next_request_id: i32,
}

impl GlobalState {
    fn new(
        sender: crossbeam_channel::Sender<Message>,
        cancel_receiver: crossbeam_channel::Receiver<Message>,
    ) -> Self {
        Self {
            sender,
            host: AnalysisHost::new(),
            open_files: HashMap::new(),
            pending_diagnostics: HashMap::new(),
            workspace_roots: Vec::new(),
            watch_supported: false,
            toolchain_config: ToolchainConfig::default(),
            toolchain: None,
            compiler_diagnostics: Arc::new(Mutex::new(HashMap::new())),
            save_generations: Arc::new(Mutex::new(HashMap::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
            cancel_receiver,
            next_request_id: 0,
        }
    }

    /// Never-die wrapper: panics are logged, requests still get a response.
    fn dispatch(&mut self, msg: Message) {
        let request_id = match &msg {
            Message::Request(req) => Some(req.id.clone()),
            _ => None,
        };
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle_message(msg)));
        if let Err(panic) = result {
            eprintln!(
                "compact-analyzer: panic while handling message: {}",
                panic_message(panic.as_ref())
            );
            if let Some(id) = request_id {
                self.respond(Response::new_err(
                    id,
                    INTERNAL_ERROR,
                    "internal error (panic); see server logs".to_string(),
                ));
            }
        }
    }

    fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::Request(req) => self.handle_request(req),
            Message::Notification(not) => self.handle_notification(not),
            // We do send server→client requests now (e.g. `registerCapability`
            // for file watching); their responses carry no data we need, so
            // drop them here.
            Message::Response(_) => {}
        }
    }

    fn handle_request(&mut self, req: Request) {
        match req.method.as_str() {
            "textDocument/definition" => {
                let result = serde_json::from_value::<lsp_types::GotoDefinitionParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let doc = params.text_document_position_params;
                        self.position_to_file_position(&doc.text_document.uri, doc.position)
                    })
                    .and_then(|pos| analyzer_ide::goto_definition(&mut self.host, pos))
                    .and_then(|target| lsp_utils::nav_target_to_location(&mut self.host, &target));
                self.respond_ok(req.id, result);
            }
            "textDocument/references" => {
                let recv = self.cancel_receiver.clone();
                let should_continue = || recv.is_empty();
                let result = serde_json::from_value::<lsp_types::ReferenceParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let include_decl = params.context.include_declaration;
                        let pos = self.position_to_file_position(
                            &params.text_document_position.text_document.uri,
                            params.text_document_position.position,
                        )?;
                        analyzer_ide::find_references_cancellable(
                            &mut self.host,
                            pos,
                            include_decl,
                            &should_continue,
                        )
                    })
                    .map(|targets| {
                        targets
                            .iter()
                            .filter_map(|t| lsp_utils::nav_target_to_location(&mut self.host, t))
                            .collect::<Vec<_>>()
                    });
                self.respond_ok(req.id, result);
            }
            "textDocument/hover" => {
                let result = serde_json::from_value::<lsp_types::HoverParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let doc = params.text_document_position_params;
                        let pos =
                            self.position_to_file_position(&doc.text_document.uri, doc.position)?;
                        let hover = analyzer_ide::hover(&mut self.host, pos)?;
                        Some(lsp_types::Hover {
                            contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                                kind: lsp_types::MarkupKind::Markdown,
                                value: hover.markdown,
                            }),
                            range: None,
                        })
                    });
                self.respond_ok(req.id, result);
            }
            "textDocument/documentSymbol" => {
                let result = serde_json::from_value::<lsp_types::DocumentSymbolParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let file = *self.open_files.get(&params.text_document.uri)?;
                        let analysis = self.host.analyze(file)?;
                        let line_index = analysis.line_index.clone();
                        let symbols = analyzer_ide::document_symbols(&mut self.host, file);
                        Some(
                            symbols
                                .into_iter()
                                .map(|s| doc_symbol_to_lsp(&line_index, s))
                                .collect::<Vec<_>>(),
                        )
                    });
                self.respond_ok(
                    req.id,
                    result.map(lsp_types::DocumentSymbolResponse::Nested),
                );
            }
            "textDocument/rename" => {
                let Ok(params) = serde_json::from_value::<lsp_types::RenameParams>(req.params)
                else {
                    self.respond(Response::new_err(
                        req.id,
                        INTERNAL_ERROR,
                        "bad params".into(),
                    ));
                    return;
                };
                let uri = params.text_document_position.text_document.uri.clone();
                let Some(pos) =
                    self.position_to_file_position(&uri, params.text_document_position.position)
                else {
                    self.respond_ok(req.id, Option::<lsp_types::WorkspaceEdit>::None);
                    return;
                };
                let recv = self.cancel_receiver.clone();
                let should_continue = || recv.is_empty();
                match analyzer_ide::rename_cancellable(
                    &mut self.host,
                    pos,
                    &params.new_name,
                    &should_continue,
                ) {
                    Ok(edits) => {
                        let mut changes: HashMap<Url, Vec<lsp_types::TextEdit>> = HashMap::new();
                        for edit in edits {
                            let Some(target_uri) =
                                Url::from_file_path(self.host.vfs().path(edit.file)).ok()
                            else {
                                continue;
                            };
                            let Some(analysis) = self.host.analyze(edit.file) else {
                                continue;
                            };
                            changes
                                .entry(target_uri)
                                .or_default()
                                .push(lsp_types::TextEdit {
                                    range: lsp_utils::range_to_lsp(
                                        &analysis.line_index,
                                        edit.range,
                                    ),
                                    new_text: edit.new_text,
                                });
                        }
                        self.respond_ok(
                            req.id,
                            Some(lsp_types::WorkspaceEdit {
                                changes: Some(changes),
                                ..Default::default()
                            }),
                        );
                    }
                    Err(err) => {
                        self.respond(Response::new_err(req.id, REQUEST_FAILED, err.to_string()))
                    }
                }
            }
            "textDocument/completion" => {
                let result = serde_json::from_value::<lsp_types::CompletionParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let doc = params.text_document_position;
                        self.position_to_file_position(&doc.text_document.uri, doc.position)
                    })
                    .map(|pos| {
                        analyzer_ide::completion(&mut self.host, pos)
                            .into_iter()
                            .map(lsp_utils::completion_item_to_lsp)
                            .collect::<Vec<_>>()
                    })
                    .map(lsp_types::CompletionResponse::Array);
                self.respond_ok(req.id, result);
            }
            "textDocument/semanticTokens/full" => {
                let result = serde_json::from_value::<lsp_types::SemanticTokensParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let file = *self.open_files.get(&params.text_document.uri)?;
                        let li = self.host.analyze(file)?.line_index.clone();
                        let tokens = analyzer_ide::semantic_tokens(&mut self.host, file);
                        Some(lsp_types::SemanticTokens {
                            result_id: None,
                            data: crate::semantic_tokens_legend::encode_semantic_tokens(
                                &tokens, &li,
                            ),
                        })
                    })
                    .map(lsp_types::SemanticTokensResult::Tokens);
                self.respond_ok(req.id, result);
            }
            "textDocument/foldingRange" => {
                let result = serde_json::from_value::<lsp_types::FoldingRangeParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let file = *self.open_files.get(&params.text_document.uri)?;
                        let li = self.host.analyze(file)?.line_index.clone();
                        let folds = analyzer_ide::folding_ranges(&mut self.host, file);
                        Some(
                            folds
                                .into_iter()
                                .filter_map(|f| fold_to_lsp(&li, f))
                                .collect::<Vec<_>>(),
                        )
                    });
                self.respond_ok(req.id, result);
            }
            "textDocument/selectionRange" => {
                let result = serde_json::from_value::<lsp_types::SelectionRangeParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let file = *self.open_files.get(&params.text_document.uri)?;
                        let analysis = self.host.analyze(file)?;
                        let li = analysis.line_index.clone();
                        let offsets: Vec<analyzer_core::TextSize> = params
                            .positions
                            .iter()
                            .filter_map(|p| lsp_utils::offset_from_position(&li, *p))
                            .collect();
                        let chains = analyzer_ide::selection_ranges(&mut self.host, file, &offsets);
                        Some(
                            chains
                                .into_iter()
                                .filter_map(|chain| build_selection(&li, &chain))
                                .collect::<Vec<_>>(),
                        )
                    });
                self.respond_ok(req.id, result);
            }
            "workspace/symbol" => {
                let result = serde_json::from_value::<lsp_types::WorkspaceSymbolParams>(req.params)
                    .ok()
                    .map(|params| self.workspace_symbol_infos(&params.query));
                self.respond_ok(req.id, result.map(lsp_types::WorkspaceSymbolResponse::Flat));
            }
            _ => self.respond(Response::new_err(
                req.id,
                METHOD_NOT_FOUND,
                format!("method not supported: {}", req.method),
            )),
        }
    }

    #[allow(deprecated)] // lsp_types::SymbolInformation::deprecated
    fn workspace_symbol_infos(&mut self, query: &str) -> Vec<lsp_types::SymbolInformation> {
        let items = analyzer_ide::workspace_symbols(&mut self.host, query);
        let mut out = Vec::new();
        for it in items {
            let Ok(uri) = Url::from_file_path(self.host.vfs().path(it.file)) else {
                continue;
            };
            let Some(analysis) = self.host.analyze(it.file) else {
                continue;
            };
            out.push(lsp_types::SymbolInformation {
                name: it.name,
                kind: symbol_kind_to_lsp(it.kind),
                tags: None,
                deprecated: None,
                location: lsp_types::Location {
                    uri,
                    range: lsp_utils::range_to_lsp(&analysis.line_index, it.name_range),
                },
                container_name: it.container,
            });
        }
        out
    }

    /// (uri, Position) → FilePosition for an OPEN document. Unknown or
    /// unopened documents return None (the request answers null).
    fn position_to_file_position(
        &mut self,
        uri: &Url,
        position: lsp_types::Position,
    ) -> Option<analyzer_core::FilePosition> {
        let file = *self.open_files.get(uri)?;
        let analysis = self.host.analyze(file)?;
        let offset = lsp_utils::offset_from_position(&analysis.line_index, position)?;
        Some(analyzer_core::FilePosition { file, offset })
    }

    fn respond_ok<T: serde::Serialize>(&self, id: lsp_server::RequestId, result: T) {
        match serde_json::to_value(result) {
            Ok(value) => self.respond(Response::new_ok(id, value)),
            Err(err) => self.respond(Response::new_err(
                id,
                INTERNAL_ERROR,
                format!("failed to serialize response: {err}"),
            )),
        }
    }

    fn handle_notification(&mut self, not: Notification) {
        match not.method.as_str() {
            "textDocument/didOpen" => {
                if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(not.params)
                {
                    self.did_open(params);
                }
            }
            "textDocument/didChange" => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(not.params)
                {
                    self.did_change(params);
                }
            }
            "textDocument/didClose" => {
                if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(not.params)
                {
                    self.did_close(params);
                }
            }
            "workspace/didChangeWatchedFiles" => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeWatchedFilesParams>(not.params)
                {
                    self.did_change_watched_files(params);
                }
            }
            "textDocument/didSave" => {
                if let Ok(params) = serde_json::from_value::<DidSaveTextDocumentParams>(not.params)
                {
                    self.did_save(params);
                }
            }
            // Recognized but irrelevant.
            "initialized" | "$/setTrace" | "$/cancelRequest" => {}
            other => eprintln!("compact-analyzer: ignoring notification {other}"),
        }
    }

    fn did_open(&mut self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(path) = lsp_utils::abs_path_from_uri(&uri) else {
            eprintln!("compact-analyzer: ignoring non-file document {uri}");
            return;
        };
        let file = self.host.vfs_mut().file_id(&path);
        self.host.vfs_mut().set_overlay(
            file,
            params.text_document.text,
            params.text_document.version,
        );
        self.open_files.insert(uri, file);
        self.host.index_file(file);
        self.republish_open_diagnostics();
    }

    fn did_change(&mut self, params: DidChangeTextDocumentParams) {
        let Some(&file) = self.open_files.get(&params.text_document.uri) else {
            return;
        };
        // FULL sync: the last change contains the entire new document text.
        let Some(change) = params.content_changes.into_iter().next_back() else {
            return;
        };
        if change.range.is_some() {
            eprintln!(
                "compact-analyzer: ignoring incremental didChange (server advertises full sync)"
            );
            return;
        }
        self.host
            .vfs_mut()
            .set_overlay(file, change.text, params.text_document.version);
        self.host.index_file(file);
        self.republish_open_diagnostics();
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) {
        let Some(file) = self.open_files.remove(&params.text_document.uri) else {
            return;
        };
        self.host.vfs_mut().remove_overlay(file);
        self.pending_diagnostics.remove(&file);
        // Treat close as a supersession event: bump the save generation FIRST
        // (monotonically, keeping the entry — a later reopen+save must never
        // reset the counter to a value an in-flight pre-close worker could
        // coincidentally still match). A compile that finishes after this then
        // sees `gens[file] != its captured generation` inside its critical
        // section and drops without storing or publishing — so it can't
        // re-insert a stale set that would resurface on reopen (`Vfs` interns
        // the same path to the same `FileId`). Then drop the stored set.
        // Separate lock acquisitions, same order the worker uses
        // (`save_generations` → `compiler_diagnostics`).
        lock(&self.save_generations)
            .entry(file)
            .and_modify(|g| *g += 1);
        lock(&self.compiler_diagnostics).remove(&file);
        // Clear this document's diagnostics in the editor.
        self.publish(PublishDiagnosticsParams {
            uri: params.text_document.uri,
            diagnostics: vec![],
            version: None,
        });
    }

    /// `didSave` (on the MAIN thread): if a toolchain is present and
    /// compile-on-save is enabled, launch an off-loop compile of the saved
    /// file and merge its diagnostics with the native ones. All host reads
    /// (source, line index, native diagnostics, import search path) happen
    /// here — the worker cannot touch `host` — and are handed to the worker as
    /// owned, `Send` values.
    fn did_save(&mut self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        // Only compile files the editor actually has open (we have their source).
        let Some(&file) = self.open_files.get(&uri) else {
            return;
        };
        // Toolchain optionality (hard invariant): no toolchain OR toggle off →
        // a clean no-op. No error spam, no per-request error.
        let Some(toolchain) = self.toolchain.clone() else {
            return;
        };
        if !self.toolchain_config.compile_on_save {
            return;
        }
        // didSave compiles the ON-DISK file; a non-file URI has no disk path.
        let Some(disk_path) = lsp_utils::abs_path_from_uri(&uri) else {
            return;
        };
        // Current in-editor source (for `locate`) + line index (for byte→UTF-16
        // conversion). Single-threaded main loop, so these are consistent.
        let Some(source) = self.host.vfs_mut().read(file) else {
            return;
        };
        let Some(analysis) = self.host.analyze(file) else {
            return;
        };
        let line_index = analysis.line_index.clone();
        // Native diagnostics, computed on the main thread (the worker can't read
        // `host`): both the parser set and the import/include resolution set,
        // exactly as `publish_file_diagnostics` builds them.
        let mut native: Vec<lsp_types::Diagnostic> = analysis
            .diagnostics
            .iter()
            .map(|d| lsp_utils::diagnostic_to_lsp(d, &line_index, &uri))
            .collect();
        drop(analysis);
        for d in self.host.resolution_diagnostics(file) {
            native.push(lsp_utils::diagnostic_to_lsp(&d, &line_index, &uri));
        }
        let search_path = self.host.import_search_path();
        // Bump this file's save generation under lock; the worker re-checks it
        // (under the same lock) before storing/publishing, so a stale worker
        // drops its result and can't clobber a newer one.
        let generation = {
            let mut gens = lock(&self.save_generations);
            let g = gens.entry(file).or_insert(0);
            *g += 1;
            *g
        };

        let job = CompileJob {
            sender: self.sender.clone(),
            compiler_diagnostics: Arc::clone(&self.compiler_diagnostics),
            save_generations: Arc::clone(&self.save_generations),
            shutdown: Arc::clone(&self.shutdown),
            toolchain,
            disk_path,
            source: source.to_string(),
            line_index,
            uri,
            search_path,
            native,
            file,
            generation,
        };
        // Off the main loop. The body has its OWN catch_unwind because it runs
        // OUTSIDE `dispatch`'s per-message guard: a broken/adversarial compiler
        // or a panic in position mapping must never crash the server. The
        // handle is intentionally dropped (never joined) — shutdown is bounded
        // by the sender-drop mechanism, not by joining workers.
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run()));
            if let Err(panic) = result {
                eprintln!(
                    "compact-analyzer: panic in compile worker: {}",
                    panic_message(panic.as_ref())
                );
            }
        });
    }

    /// Never-die wrapper around diagnostics publication: parsing and
    /// position conversion run on arbitrary user text — a panic here must
    /// not kill the server. The inner function removes each file's pending
    /// deadline before publishing it, so a panicking flush cannot spin the
    /// main loop.
    fn flush_pending_diagnostics(&mut self) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.flush_pending_diagnostics_inner()
        }));
        if let Err(panic) = result {
            eprintln!(
                "compact-analyzer: panic while publishing diagnostics: {}",
                panic_message(panic.as_ref())
            );
        }
    }

    fn flush_pending_diagnostics_inner(&mut self) {
        let now = Instant::now();
        let due: Vec<FileId> = self
            .pending_diagnostics
            .iter()
            .filter(|&(_, &deadline)| deadline <= now)
            .map(|(&f, _)| f)
            .collect();
        for file in due {
            self.pending_diagnostics.remove(&file);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.publish_file_diagnostics(file)
            }));
            if let Err(panic) = result {
                eprintln!(
                    "compact-analyzer: panic while publishing diagnostics for one file: {}",
                    panic_message(panic.as_ref())
                );
            }
        }
    }

    fn publish_file_diagnostics(&mut self, file: FileId) {
        let Some(uri) = self
            .open_files
            .iter()
            .find(|&(_, &f)| f == file)
            .map(|(uri, _)| uri.clone())
        else {
            return; // closed before the debounce fired
        };
        let Some(analysis) = self.host.analyze(file) else {
            return;
        };
        let version = self.host.vfs().overlay_version(file);
        let mut native: Vec<lsp_types::Diagnostic> = analysis
            .diagnostics
            .iter()
            .map(|d| lsp_utils::diagnostic_to_lsp(d, &analysis.line_index, &uri))
            .collect();
        for d in self.host.resolution_diagnostics(file) {
            native.push(lsp_utils::diagnostic_to_lsp(&d, &analysis.line_index, &uri));
        }
        // Re-merge the file's stored compiler diagnostics (from the last save)
        // so this debounced NATIVE republish doesn't wipe them.
        let compiler = lock(&self.compiler_diagnostics)
            .get(&file)
            .cloned()
            .unwrap_or_default();
        let diagnostics = merge_diagnostics(&native, &compiler);
        self.publish(PublishDiagnosticsParams {
            uri,
            diagnostics,
            version,
        });
    }

    fn publish(&self, params: PublishDiagnosticsParams) {
        self.send_notification::<lsp_types::notification::PublishDiagnostics>(params);
    }

    fn send_notification<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        let not = Notification::new(N::METHOD.to_string(), params);
        let _ = self.sender.send(Message::Notification(not));
    }

    fn respond(&self, response: Response) {
        let _ = self.sender.send(Message::Response(response));
    }

    /// Registers a `**/*.compact` file watcher via `client/registerCapability`.
    fn register_file_watcher(&mut self) {
        let registration = lsp_types::Registration {
            id: "watch-compact".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: serde_json::to_value(
                lsp_types::DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![lsp_types::FileSystemWatcher {
                        glob_pattern: lsp_types::GlobPattern::String("**/*.compact".to_string()),
                        kind: None,
                    }],
                },
            )
            .ok(),
        };
        self.send_request::<lsp_types::request::RegisterCapability>(
            lsp_types::RegistrationParams {
                registrations: vec![registration],
            },
        );
    }

    /// Sends a server→client request. The client's response is ignored (the
    /// main loop drops `Message::Response`).
    fn send_request<R: lsp_types::request::Request>(&mut self, params: R::Params) {
        self.next_request_id += 1;
        let req = Request::new(
            lsp_server::RequestId::from(self.next_request_id),
            R::METHOD.to_string(),
            params,
        );
        let _ = self.sender.send(Message::Request(req));
    }

    fn did_change_watched_files(&mut self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            let Some(path) = lsp_utils::abs_path_from_uri(&change.uri) else {
                continue;
            };
            let file = self.host.vfs_mut().file_id(&path);
            if change.typ == lsp_types::FileChangeType::DELETED {
                self.host.remove_workspace_file(file);
            } else {
                self.host.vfs_mut().invalidate_disk(file);
                self.host.index_file(file);
            }
        }
        self.republish_open_diagnostics();
    }

    /// Re-schedules diagnostics for every open file (a cross-file change may
    /// have altered their resolution results).
    fn republish_open_diagnostics(&mut self) {
        let deadline = Instant::now() + DEBOUNCE;
        let open: Vec<FileId> = self.open_files.values().copied().collect();
        for f in open {
            self.pending_diagnostics.insert(f, deadline);
        }
    }
}

/// One compile-on-save job, moved wholesale into a worker thread. Every field
/// is owned/`Send`; grouping them into a struct (rather than a 13-argument
/// function) keeps the spawn site legible and sidesteps `too_many_arguments`.
struct CompileJob {
    sender: crossbeam_channel::Sender<Message>,
    compiler_diagnostics: CompilerDiagnostics,
    save_generations: SaveGenerations,
    shutdown: Arc<AtomicBool>,
    toolchain: Toolchain,
    disk_path: PathBuf,
    /// The in-editor source at save time — `locate` maps compiler `(line,col)`
    /// against this, not against a fresh disk read (they match on save, and
    /// this keeps position mapping consistent with what the editor shows).
    source: String,
    line_index: Arc<LineIndex>,
    uri: Url,
    search_path: Vec<PathBuf>,
    /// Native diagnostics, pre-computed on the main thread.
    native: Vec<lsp_types::Diagnostic>,
    file: FileId,
    generation: u64,
}

impl CompileJob {
    /// Runs the compile, then (if still current) stores + publishes the merged
    /// set. Called inside the worker's own `catch_unwind`. Publishes directly
    /// via the cloned `sender`, bypassing the main-loop debounce.
    fn run(self) {
        // Fresh scratch dir, auto-removed when `scratch` drops at function end.
        let Ok(scratch) = tempfile::tempdir() else {
            return; // can't make a scratch dir; nothing actionable to do
        };
        let outcome = analyzer_toolchain::compile_file(
            &self.toolchain,
            &self.disk_path,
            scratch.path(),
            &self.search_path,
            COMPILE_TIMEOUT,
        );
        let compiler =
            build_compiler_diagnostics(&outcome, &self.source, &self.line_index, &self.disk_path);

        // Supersession + store, atomic w.r.t. `did_save`/`did_close` generation
        // bumps (see `store_if_current`). If this result is no longer current
        // (a newer save, or a close) or we're shutting down, drop it: store
        // nothing, publish nothing.
        if !store_if_current(
            &self.save_generations,
            &self.compiler_diagnostics,
            &self.shutdown,
            self.file,
            self.generation,
            &compiler,
        ) {
            return;
        }

        // Merge OUTSIDE the locks; never hold a lock across a `send`.
        let merged = merge_diagnostics(&self.native, &compiler);
        let params = PublishDiagnosticsParams {
            uri: self.uri,
            diagnostics: merged,
            version: None,
        };
        let not = Notification::new("textDocument/publishDiagnostics".to_string(), params);
        let _ = self.sender.send(Message::Notification(not));
    }
}

/// The supersession + store critical section, factored out of
/// [`CompileJob::run`] so the guard is deterministically unit-testable.
///
/// Holds `save_generations` across the `compiler_diagnostics` store so a
/// concurrent `did_save`/`did_close` generation bump can't slip between the
/// currency check and the store (TOCTOU-closed). Lock order is always
/// `save_generations` → `compiler_diagnostics`, matching every other acquirer.
///
/// Returns `true` iff `generation` is still current for `file` and we are not
/// shutting down — in which case the compiler set was stored (or removed, if
/// empty) and the caller should publish. Returns `false` (touching nothing)
/// when superseded by a newer save, invalidated by a close (which bumps the
/// generation), or shutting down; the caller must then drop its result.
fn store_if_current(
    save_generations: &Mutex<HashMap<FileId, u64>>,
    compiler_diagnostics: &Mutex<HashMap<FileId, Vec<lsp_types::Diagnostic>>>,
    shutdown: &AtomicBool,
    file: FileId,
    generation: u64,
    compiler: &[lsp_types::Diagnostic],
) -> bool {
    let gens = lock(save_generations);
    if gens.get(&file).copied() != Some(generation) {
        return false; // superseded by a newer save, or invalidated by a close
    }
    if shutdown.load(Ordering::Acquire) {
        return false; // shutting down; publish nothing, drop the sender
    }
    let mut store = lock(compiler_diagnostics);
    if compiler.is_empty() {
        store.remove(&file);
    } else {
        store.insert(file, compiler.to_vec());
    }
    true
}

/// Turns a [`CompileOutcome`] into tagged (`source = "compactc"`) LSP
/// diagnostics.
///
/// - `Ok` → empty (clears any prior compiler set for this file).
/// - `CompileError` → one squiggle per diagnostic attributable to the saved
///   file (basename match + a resolvable position); every un-attributable one
///   (a dependency basename, or an unresolvable position) collapses into a
///   single generic file-top diagnostic. Exit-255-with-nothing-structured
///   still yields a generic "compilation failed" so a real failure is never
///   silently cleared.
/// - `TimedOut` / `InvocationError` → one generic file-top diagnostic (never
///   a storm).
fn build_compiler_diagnostics(
    outcome: &CompileOutcome,
    source: &str,
    line_index: &LineIndex,
    disk_path: &Path,
) -> Vec<lsp_types::Diagnostic> {
    match outcome.status {
        CompileStatus::Ok => Vec::new(),
        CompileStatus::CompileError => {
            let parsed = analyzer_toolchain::parse_compiler_stderr(&outcome.stderr);
            let saved_basename = disk_path.file_name().and_then(|s| s.to_str());
            let mut attributed: Vec<lsp_types::Diagnostic> = Vec::new();
            let mut unattributed: Vec<String> = Vec::new();
            for d in &parsed.diagnostics {
                let matches_saved = saved_basename == Some(d.file_basename.as_str());
                match (
                    matches_saved,
                    analyzer_toolchain::locate(source, d.line, d.col),
                ) {
                    (true, Some(range)) => attributed.push(lsp_utils::compiler_diagnostic_to_lsp(
                        range,
                        d.message.clone(),
                        line_index,
                    )),
                    // Dependency-attributed, or a position that wouldn't resolve
                    // (line 0 / col 0 / out-of-range line): fold into the generic.
                    _ => unattributed.push(format!("{} (in {})", d.message, d.file_basename)),
                }
            }
            let mut out = attributed;
            if !unattributed.is_empty() {
                out.push(file_top_compiler_diagnostic(
                    format!("compact: {}", unattributed.join("; ")),
                    line_index,
                ));
            } else if out.is_empty() && !parsed.unparsed.is_empty() {
                out.push(file_top_compiler_diagnostic(
                    "compact: could not parse compiler output".to_string(),
                    line_index,
                ));
            }
            if out.is_empty() {
                // Exit 255 but nothing structured to show: surface a generic
                // failure rather than silently clearing diagnostics.
                out.push(file_top_compiler_diagnostic(
                    "compact: compilation failed".to_string(),
                    line_index,
                ));
            }
            out
        }
        CompileStatus::TimedOut => vec![file_top_compiler_diagnostic(
            format!(
                "compact: compiler timed out after {}s",
                COMPILE_TIMEOUT.as_secs()
            ),
            line_index,
        )],
        CompileStatus::InvocationError => vec![file_top_compiler_diagnostic(
            "compact: compiler unavailable (invocation failed)".to_string(),
            line_index,
        )],
    }
}

/// A single `source = "compactc"` diagnostic anchored at the top of the file
/// (zero-width range at offset 0), for compiler output that isn't attributable
/// to a specific span in the saved file.
fn file_top_compiler_diagnostic(message: String, line_index: &LineIndex) -> lsp_types::Diagnostic {
    lsp_utils::compiler_diagnostic_to_lsp(TextRange::empty(TextSize::new(0)), message, line_index)
}

/// Merges native diagnostics with compiler ones: all native, then every
/// compiler diagnostic whose `range` does NOT coincide with a native one
/// (span-coincidence dedup, keep native). Deterministic and order-stable.
/// Shared by the compile worker and `publish_file_diagnostics`.
fn merge_diagnostics(
    native: &[lsp_types::Diagnostic],
    compiler: &[lsp_types::Diagnostic],
) -> Vec<lsp_types::Diagnostic> {
    let mut merged = native.to_vec();
    for c in compiler {
        if !native.iter().any(|n| n.range == c.range) {
            merged.push(c.clone());
        }
    }
    merged
}

#[allow(deprecated)] // lsp_types::DocumentSymbol::deprecated
fn doc_symbol_to_lsp(
    li: &analyzer_core::LineIndex,
    symbol: analyzer_ide::DocSymbol,
) -> lsp_types::DocumentSymbol {
    lsp_types::DocumentSymbol {
        name: symbol.name,
        detail: symbol.detail,
        kind: symbol_kind_to_lsp(symbol.kind),
        tags: None,
        deprecated: None,
        range: crate::lsp_utils::range_to_lsp(li, symbol.full_range),
        selection_range: crate::lsp_utils::range_to_lsp(li, symbol.selection_range),
        children: Some(
            symbol
                .children
                .into_iter()
                .map(|c| doc_symbol_to_lsp(li, c))
                .collect(),
        ),
    }
}

fn symbol_kind_to_lsp(kind: analyzer_core::SymbolKind) -> lsp_types::SymbolKind {
    use analyzer_core::SymbolKind as K;
    match kind {
        K::Circuit | K::CircuitSig | K::Witness => lsp_types::SymbolKind::FUNCTION,
        K::Struct => lsp_types::SymbolKind::STRUCT,
        K::StructField => lsp_types::SymbolKind::FIELD,
        K::Enum => lsp_types::SymbolKind::ENUM,
        K::EnumVariant => lsp_types::SymbolKind::ENUM_MEMBER,
        K::Module => lsp_types::SymbolKind::MODULE,
        K::TypeAlias => lsp_types::SymbolKind::INTERFACE,
        K::Ledger => lsp_types::SymbolKind::PROPERTY,
        K::Contract => lsp_types::SymbolKind::INTERFACE,
        K::ContractCircuit => lsp_types::SymbolKind::METHOD,
        K::Constructor => lsp_types::SymbolKind::CONSTRUCTOR,
    }
}

/// Byte-range fold → LSP `FoldingRange`; single-line ranges are dropped.
fn fold_to_lsp(
    li: &analyzer_core::LineIndex,
    fold: analyzer_ide::FoldRange,
) -> Option<lsp_types::FoldingRange> {
    let start = li.line_col(fold.range.start());
    let end = li.line_col(fold.range.end());
    if start.line == end.line {
        return None;
    }
    let kind = match fold.kind {
        analyzer_ide::FoldKind::Imports => Some(lsp_types::FoldingRangeKind::Imports),
        analyzer_ide::FoldKind::Comment => Some(lsp_types::FoldingRangeKind::Comment),
        analyzer_ide::FoldKind::Region => Some(lsp_types::FoldingRangeKind::Region),
    };
    Some(lsp_types::FoldingRange {
        start_line: start.line,
        start_character: None,
        end_line: end.line,
        end_character: None,
        kind,
        collapsed_text: None,
    })
}

/// Innermost-first byte-range chain → nested LSP `SelectionRange`.
fn build_selection(
    li: &analyzer_core::LineIndex,
    chain: &[analyzer_core::TextRange],
) -> Option<lsp_types::SelectionRange> {
    let mut cur: Option<Box<lsp_types::SelectionRange>> = None;
    for &r in chain.iter().rev() {
        cur = Some(Box::new(lsp_types::SelectionRange {
            range: lsp_utils::range_to_lsp(li, r),
            parent: cur,
        }));
    }
    cur.map(|b| *b)
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn workspace_roots_from_params(params: &serde_json::Value) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(folders) = params.get("workspaceFolders").and_then(|v| v.as_array()) {
        for f in folders {
            if let Some(uri) = f.get("uri").and_then(|u| u.as_str())
                && let Ok(url) = Url::parse(uri)
                && let Some(p) = lsp_utils::abs_path_from_uri(&url)
            {
                roots.push(p);
            }
        }
    }
    if roots.is_empty()
        && let Some(uri) = params.get("rootUri").and_then(|u| u.as_str())
        && let Ok(url) = Url::parse(uri)
        && let Some(p) = lsp_utils::abs_path_from_uri(&url)
    {
        roots.push(p);
    }
    roots
}

/// v1 toolchain configuration surface (design §4), read from
/// `initializationOptions`. `toolchain_path`, `compile_on_save`, and
/// `formatting` are forward-declared here for Tasks 6/7/9, which will read
/// them; this task only parses and stores them.
pub(crate) struct ToolchainConfig {
    #[allow(dead_code)]
    pub toolchain_path: Option<PathBuf>,
    #[allow(dead_code)]
    pub compile_on_save: bool,
    #[allow(dead_code)]
    pub formatting: bool,
}

impl Default for ToolchainConfig {
    fn default() -> Self {
        Self {
            toolchain_path: None,
            compile_on_save: true,
            formatting: true,
        }
    }
}

fn toolchain_config_from(options: Option<&serde_json::Value>) -> ToolchainConfig {
    let mut config = ToolchainConfig::default();
    let Some(opts) = options else {
        return config;
    };
    if let Some(path) = opts.get("toolchainPath").and_then(|v| v.as_str()) {
        config.toolchain_path = Some(PathBuf::from(path));
    }
    if let Some(flag) = opts.get("compileOnSave").and_then(|v| v.as_bool()) {
        config.compile_on_save = flag;
    }
    if let Some(flag) = opts.get("formatting").and_then(|v| v.as_bool()) {
        config.formatting = flag;
    }
    config
}

fn import_search_path_from(options: Option<&serde_json::Value>) -> Vec<PathBuf> {
    if let Some(opts) = options
        && let Some(arr) = opts.get("importSearchPath").and_then(|v| v.as_array())
    {
        return arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(PathBuf::from)
            .collect();
    }
    std::env::var_os("COMPACT_PATH")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default()
}

fn client_supports_watch(params: &serde_json::Value) -> bool {
    params
        .pointer("/capabilities/workspace/didChangeWatchedFiles/dynamicRegistration")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workspace_roots_prefers_folders_then_root_uri() {
        let base = std::env::temp_dir();
        let a = base.join("wsA");
        let uri_a = Url::from_file_path(&a).unwrap();
        let params = json!({ "workspaceFolders": [{ "uri": uri_a, "name": "A" }] });
        assert_eq!(workspace_roots_from_params(&params), vec![a.clone()]);

        let params = json!({ "rootUri": uri_a });
        assert_eq!(workspace_roots_from_params(&params), vec![a]);

        assert!(workspace_roots_from_params(&json!({})).is_empty());
    }

    #[test]
    fn import_search_path_reads_options() {
        let opts = json!({ "importSearchPath": ["/libs/x", "/libs/y"] });
        assert_eq!(
            import_search_path_from(Some(&opts)),
            vec![PathBuf::from("/libs/x"), PathBuf::from("/libs/y")]
        );
    }

    #[test]
    fn toolchain_config_reads_options() {
        let opts = json!({ "toolchainPath": "/opt/compact", "compileOnSave": false });
        let config = toolchain_config_from(Some(&opts));
        assert_eq!(config.toolchain_path, Some(PathBuf::from("/opt/compact")));
        assert!(!config.compile_on_save);
        assert!(config.formatting);
    }

    #[test]
    fn toolchain_config_defaults_when_options_empty() {
        let config = toolchain_config_from(Some(&json!({})));
        assert_eq!(config.toolchain_path, None);
        assert!(config.compile_on_save);
        assert!(config.formatting);
    }

    #[test]
    fn toolchain_config_defaults_when_options_absent() {
        let config = toolchain_config_from(None);
        assert_eq!(config.toolchain_path, None);
        assert!(config.compile_on_save);
        assert!(config.formatting);
    }

    #[test]
    fn client_watch_capability_is_detected() {
        let yes = json!({ "capabilities": { "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } } });
        assert!(client_supports_watch(&yes));
        assert!(!client_supports_watch(&json!({ "capabilities": {} })));
    }

    // --- Supersession guard (the close-during-compile fix) -----------------

    #[test]
    fn store_if_current_rejects_stale_generation_after_close_bump() {
        // Regression test for close-during-compile: a compile that finishes
        // AFTER `did_close` bumped the generation must not re-insert its stale
        // set (which would resurface on reopen, since the path re-interns to
        // the same `FileId`).
        let mut vfs = analyzer_core::Vfs::new();
        let file = vfs.file_id(std::path::Path::new("/tmp/x.compact"));

        let gens: Mutex<HashMap<FileId, u64>> = Mutex::new(HashMap::from([(file, 5)]));
        let store: Mutex<HashMap<FileId, Vec<lsp_types::Diagnostic>>> = Mutex::new(HashMap::new());
        let shutdown = AtomicBool::new(false);
        let compiler = vec![lsp_types::Diagnostic::default()];

        // A worker whose captured generation (5) is still current stores its set.
        assert!(store_if_current(
            &gens, &store, &shutdown, file, 5, &compiler
        ));
        assert!(lock(&store).contains_key(&file));

        // Simulate `did_close`: clear the stored set AND bump 5 -> 6 (monotonic,
        // entry kept — exactly what `did_close` now does).
        lock(&store).remove(&file);
        lock(&gens).entry(file).and_modify(|g| *g += 1);

        // The in-flight pre-close worker (still holding generation 5) is now
        // stale: it must NOT re-insert its set and must NOT signal a publish.
        assert!(!store_if_current(
            &gens, &store, &shutdown, file, 5, &compiler
        ));
        assert!(
            !lock(&store).contains_key(&file),
            "a stale worker must not re-insert compiler diagnostics after close"
        );
    }

    #[test]
    fn store_if_current_rejects_during_shutdown() {
        let mut vfs = analyzer_core::Vfs::new();
        let file = vfs.file_id(std::path::Path::new("/tmp/y.compact"));
        let gens: Mutex<HashMap<FileId, u64>> = Mutex::new(HashMap::from([(file, 1)]));
        let store: Mutex<HashMap<FileId, Vec<lsp_types::Diagnostic>>> = Mutex::new(HashMap::new());
        let shutdown = AtomicBool::new(true);
        let compiler = vec![lsp_types::Diagnostic::default()];

        assert!(!store_if_current(
            &gens, &store, &shutdown, file, 1, &compiler
        ));
        assert!(lock(&store).is_empty(), "no store/publish during shutdown");
    }

    #[test]
    fn did_close_bumps_generation_and_clears_compiler_set() {
        let (tx, _keep_rx) = crossbeam_channel::unbounded::<Message>();
        let (_keep_tx, rx) = crossbeam_channel::unbounded::<Message>();
        let mut state = GlobalState::new(tx, rx);

        let uri = Url::parse("file:///tmp/z.compact").unwrap();
        let path = lsp_utils::abs_path_from_uri(&uri).unwrap();
        let file = state.host.vfs_mut().file_id(&path);
        state.open_files.insert(uri.clone(), file);
        // A prior save left generation 3 and a stored compiler set.
        lock(&state.save_generations).insert(file, 3);
        lock(&state.compiler_diagnostics).insert(file, vec![lsp_types::Diagnostic::default()]);

        state.did_close(DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
        });

        // Close bumped the generation monotonically (entry KEPT, not removed)
        // and cleared the stored compiler set — so a compile finishing after
        // this close is superseded and reopen sees no stale compiler set.
        assert_eq!(lock(&state.save_generations).get(&file).copied(), Some(4));
        assert!(!lock(&state.compiler_diagnostics).contains_key(&file));
    }
}
