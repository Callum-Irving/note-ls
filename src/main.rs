use tokio::sync::Mutex;
use tower_lsp::{
    jsonrpc::Result,
    lsp_types::{
        CompletionOptions, DidChangeTextDocumentParams, InitializeParams, InitializeResult,
        InitializedParams, MessageType, ServerCapabilities, TextDocumentContentChangeEvent,
        TextDocumentSyncCapability, TextDocumentSyncKind, Url,
    },
    Client, LanguageServer, LspService, Server,
};

struct Files {
    files: Vec<File>,
}

impl Files {
    /// Find file with matching uri.
    pub fn get_file(&mut self, uri: Url) -> Option<&mut File> {
        self.files.iter_mut().find(|f| f.uri == uri)
    }
}

struct File {
    uri: Url,
    text: String,
}

impl File {
    pub fn update(&mut self, changes: Vec<TextDocumentContentChangeEvent>) {
        todo!()
    }
}

struct MarkdownLanguageServer {
    client: Client,
    files: Mutex<Files>,
}

impl MarkdownLanguageServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            files: Mutex::new(Files { files: vec![] }),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for MarkdownLanguageServer {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["[[".to_string()]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "server initialized!")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_change(&self, request: DidChangeTextDocumentParams) {
        let modified_document = request.text_document.uri;
        let mut state = self.files.lock().await;
        let file = state
            .get_file(modified_document)
            .expect("File could not be found");

        file.update(request.content_changes);
    }
}

#[tokio::main]
async fn main() {
    println!("Hello, world!");
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| MarkdownLanguageServer::new(client));
    Server::new(stdin, stdout, socket).serve(service).await;
}
