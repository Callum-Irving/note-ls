use std::{collections::HashMap, ffi::OsStr, path::PathBuf, process::Command};

use tokio::sync::Mutex;
use tower_lsp::{
    jsonrpc::{Error, ErrorCode, Result},
    lsp_types::{
        CompletionItem, CompletionItemKind, CompletionList, CompletionOptions, CompletionParams,
        CompletionResponse, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
        DidOpenTextDocumentParams, GotoDefinitionParams, GotoDefinitionResponse, InitializeParams,
        InitializeResult, InitializedParams, MessageType, Position, ServerCapabilities,
        TextDocumentContentChangeEvent, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
        WorkDoneProgressOptions,
    },
    Client, LanguageServer, LspService, Server,
};
use walkdir::WalkDir;

/// Get the word in `document` at position `cursor_pos`. Cut off word at cursor
/// position.
///
/// ## Example:
/// ```
/// let doc = "this is a sentence";
/// let position = Position { line: 0, character: 1 };
/// let curr_word = get_current_word(doc, position).unwrap();
/// assert_eq!(curr_word, "t");
/// ```
fn get_current_word(document: &str, cursor_pos: Position) -> Option<&str> {
    let line = document.lines().nth(cursor_pos.line as usize)?;
    let character = cursor_pos.character as usize;

    // Go to position at cursor_position.character
    // Go backwards until end of iterator or whitespace, save index
    let backwards_chars = line[..character]
        .chars()
        .rev()
        .take_while(|c| !c.is_whitespace())
        .count();

    let start_word = character - backwards_chars;

    Some(&line[start_word..character])
}

struct Files {
    files: HashMap<Url, File>,
}

impl Files {
    /// Add new file
    pub fn add_file(&mut self, uri: Url, content: File) {
        self.files.insert(uri, content);
    }

    /// Find file with matching uri.
    pub fn get_file_mut(&mut self, uri: &Url) -> Option<&mut File> {
        self.files.get_mut(uri)
    }

    pub fn get_file(&self, uri: &Url) -> Option<&File> {
        self.files.get(uri)
    }

    pub fn remove_file(&mut self, uri: &Url) {
        self.files.remove(uri);
    }
}

#[derive(Clone)]
struct File {
    content: String,
}

impl File {
    pub fn new(content: String) -> Self {
        Self { content }
    }

    /// Overwrite current content with `new_content`.
    pub fn overwrite(&mut self, new_content: String) {
        self.content = new_content;
    }

    pub fn update(&mut self, changes: Vec<TextDocumentContentChangeEvent>) {
        todo!("implement incremental document synchronization")
    }
}

// TODO: Implement incremental document synchronization instead of full.
struct MarkdownLanguageServer {
    client: Client,
    files: Mutex<Files>,
    current_file: Mutex<Option<Url>>,
    preview_server: Mutex<aurelius::Server>,
}

impl MarkdownLanguageServer {
    pub fn new(client: Client) -> Self {
        let mut preview_server =
            aurelius::Server::bind("localhost:0").expect("Couldn't start preview server");

        // Use MD4C as rendered
        let mut md2html = Command::new("/home/callum/Projects/note-ls/md4c/md2html/md2html");
        md2html.arg("--flatex-math");
        preview_server.set_external_renderer(md2html);

        Self {
            client,
            files: Mutex::new(Files {
                files: HashMap::new(),
            }),
            current_file: Mutex::new(None),
            preview_server: Mutex::new(preview_server),
        }
    }

    pub async fn get_current_file_contents(&self) -> Option<File> {
        let current_file = self.current_file.lock().await;
        let c2 = current_file.clone()?;
        let lock = self.files.lock().await;
        let c = lock.get_file(&c2)?;
        let thing = c.clone();
        Some(thing)
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for MarkdownLanguageServer {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        // TODO: Client must support goto definition link

        // Open preview in browser
        let mut preview_server = self.preview_server.lock().await;
        preview_server.set_highlight_theme("github".to_string());
        preview_server
            .open_browser()
            .map_err(|_| Error::new(ErrorCode::InternalError))?;

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["[[".to_string()]),
                    resolve_provider: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    all_commit_characters: None,
                }),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "mdls language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        // TODO: Is there any shutdown logic required?
        Ok(())
    }

    async fn did_open(&self, request: DidOpenTextDocumentParams) {
        let mut state = self.files.lock().await;
        state.add_file(
            request.text_document.uri.clone(),
            File::new(request.text_document.text.clone()),
        );

        let mut current_file = self.current_file.lock().await;
        *current_file = Some(request.text_document.uri);

        // TODO: Open preview in browser
        self.preview_server
            .lock()
            .await
            .send(request.text_document.text)
            .expect("Couldn't send preview to server");
    }

    async fn did_change(&self, mut request: DidChangeTextDocumentParams) {
        debug_assert!(request.content_changes.len() > 0);

        let mut state = self.files.lock().await;
        let Some(file) = state.get_file_mut(&request.text_document.uri) else { return; };
        let last_index = request.content_changes.len() - 1;
        let new_content = request.content_changes.swap_remove(last_index).text;
        file.overwrite(new_content.clone());

        let mut current_file = self.current_file.lock().await;
        *current_file = Some(request.text_document.uri);

        // Update preview in browser
        self.preview_server
            .lock()
            .await
            .send(new_content)
            .expect("Couldn't send preview to server");
    }

    async fn did_close(&self, request: DidCloseTextDocumentParams) {
        let mut state = self.files.lock().await;
        state.remove_file(&request.text_document.uri);

        // TODO: Close preview in browser
        // Maybe switch to current document instead?
    }

    // TODO: Filter files as user types more characters.
    async fn completion(&self, request: CompletionParams) -> Result<Option<CompletionResponse>> {
        // Get current location in file
        let state = self.files.lock().await;
        let file = state
            .get_file(&request.text_document_position.text_document.uri)
            .ok_or(Error::new(ErrorCode::InvalidParams))?;
        let pos = request.text_document_position.position;

        let current_word =
            get_current_word(&file.content, pos).ok_or(Error::new(ErrorCode::InvalidParams))?;

        self.client
            .log_message(MessageType::INFO, format!("Current word: {}", current_word))
            .await;

        if current_word.starts_with("[[") && !current_word.ends_with(']') {
            // Get all files in currrent dir or nested dirs that end with .md other than self.
            let current_path = self
                .current_file
                .lock()
                .await
                .clone()
                .ok_or(Error::new(ErrorCode::InternalError))?;
            let path = PathBuf::from(current_path.path());
            let path_parent = path.parent().ok_or(Error::new(ErrorCode::InternalError))?;

            let files = WalkDir::new(path_parent)
                .sort_by(|a, b| a.depth().cmp(&b.depth())) // Not working
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension() == Some(OsStr::new("md")))
                .map(|e| CompletionItem {
                    label: e
                        .path()
                        .strip_prefix(path_parent)
                        .unwrap()
                        .to_string_lossy()
                        .into(),
                    kind: Some(CompletionItemKind::FILE),
                    ..CompletionItem::default()
                })
                .collect::<Vec<CompletionItem>>();

            Ok(Some(CompletionResponse::List(CompletionList {
                is_incomplete: false,
                items: files,
            })))
        } else {
            Ok(None)
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        todo!()
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| MarkdownLanguageServer::new(client));
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_current_word_works() {
        let doc = "this is a sentence\nThis is another line. Here is a word.";
        let position = Position {
            line: 0,
            character: 1,
        };
        let curr_word =
            get_current_word(doc, position).expect("Getting current word returned None");
        assert_eq!(curr_word, "t");

        let position = Position {
            line: 1,
            character: 8,
        };
        let curr_word =
            get_current_word(doc, position).expect("Getting current word returned None");
        assert_eq!(curr_word, "");

        let position = Position {
            line: 0,
            character: 0,
        };
        let curr_word = get_current_word(doc, position).unwrap();
        assert_eq!(curr_word, "");
    }
}
