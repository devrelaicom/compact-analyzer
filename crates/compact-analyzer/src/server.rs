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

pub(crate) fn run() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();

    let (initialize_id, _initialize_params) = connection.initialize_start()?;
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
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
        // shutdown is intercepted in the main loop; nothing else is
        // supported in M1.
        self.respond(Response::new_err(
            req.id,
            METHOD_NOT_FOUND,
            format!("method not supported: {}", req.method),
        ));
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
            let Some(uri) = self
                .open_files
                .iter()
                .find(|&(_, &f)| f == file)
                .map(|(uri, _)| uri.clone())
            else {
                continue; // closed before the debounce fired
            };
            let Some(analysis) = self.host.analyze(file) else {
                continue;
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

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}
