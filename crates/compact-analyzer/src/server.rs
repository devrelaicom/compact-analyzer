//! The LSP server: synchronous main loop over stdio.
//!
//! Never-die contract: every message dispatch runs under catch_unwind; a
//! panicked request gets an InternalError response and the loop continues.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use analyzer_core::{AnalysisHost, FileId};
use crossbeam_channel::RecvTimeoutError;
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    Url,
};

use crate::lsp_utils;

const DEBOUNCE: Duration = Duration::from_millis(150);

// JSON-RPC error codes (avoid depending on lsp-server exporting them).
const METHOD_NOT_FOUND: i32 = -32601;
const INTERNAL_ERROR: i32 = -32603;
const REQUEST_FAILED: i32 = -32803;

pub(crate) fn run() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();

    let (initialize_id, _initialize_params) = connection.initialize_start()?;
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        definition_provider: Some(lsp_types::OneOf::Left(true)),
        references_provider: Some(lsp_types::OneOf::Left(true)),
        rename_provider: Some(lsp_types::OneOf::Left(true)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(lsp_types::OneOf::Left(true)),
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

    let mut state = GlobalState::new(connection.sender.clone());
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

    loop {
        // When diagnostics are pending, wait only until the debounce
        // deadline; otherwise block until the next message.
        let msg = if let Some(deadline) = state.debounce_deadline {
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
    // channel closes). `state` holds a clone, and `connection` holds the
    // original, so both must be dropped before joining or this blocks
    // forever.
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
    /// Files with not-yet-published diagnostics.
    pending_diagnostics: HashSet<FileId>,
    /// When set, diagnostics are published once this instant passes.
    debounce_deadline: Option<Instant>,
}

impl GlobalState {
    fn new(sender: crossbeam_channel::Sender<Message>) -> Self {
        Self {
            sender,
            host: AnalysisHost::new(),
            open_files: HashMap::new(),
            pending_diagnostics: HashSet::new(),
            debounce_deadline: None,
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
            Message::Response(_) => {} // we never send server-to-client requests in M1
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
                let result = serde_json::from_value::<lsp_types::ReferenceParams>(req.params)
                    .ok()
                    .and_then(|params| {
                        let include_decl = params.context.include_declaration;
                        let pos = self.position_to_file_position(
                            &params.text_document_position.text_document.uri,
                            params.text_document_position.position,
                        )?;
                        analyzer_ide::find_references(&mut self.host, pos, include_decl)
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
                match analyzer_ide::rename(&mut self.host, pos, &params.new_name) {
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
            _ => self.respond(Response::new_err(
                req.id,
                METHOD_NOT_FOUND,
                format!("method not supported: {}", req.method),
            )),
        }
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
            // Recognized but irrelevant in M1.
            "textDocument/didSave" | "initialized" | "$/setTrace" | "$/cancelRequest" => {}
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
        self.schedule_diagnostics(file);
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
        self.schedule_diagnostics(file);
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) {
        let Some(file) = self.open_files.remove(&params.text_document.uri) else {
            return;
        };
        self.host.vfs_mut().remove_overlay(file);
        self.pending_diagnostics.remove(&file);
        if self.pending_diagnostics.is_empty() {
            self.debounce_deadline = None;
        }
        // Clear this document's diagnostics in the editor.
        self.publish(PublishDiagnosticsParams {
            uri: params.text_document.uri,
            diagnostics: vec![],
            version: None,
        });
    }

    fn schedule_diagnostics(&mut self, file: FileId) {
        self.pending_diagnostics.insert(file);
        self.debounce_deadline = Some(Instant::now() + DEBOUNCE);
    }

    /// Never-die wrapper around diagnostics publication: parsing and
    /// position conversion run on arbitrary user text — a panic here must
    /// not kill the server. The inner function clears the debounce deadline
    /// first thing, so a panicking flush cannot spin the main loop.
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
        self.debounce_deadline = None;
        let files: Vec<FileId> = self.pending_diagnostics.drain().collect();
        for file in files {
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
        let diagnostics = analysis
            .diagnostics
            .iter()
            .map(|d| lsp_utils::diagnostic_to_lsp(d, &analysis.line_index, &uri))
            .collect();
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

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}
