mod clangd;

use crate::clangd::ClangdSession;
use serde_json;
use tokio::sync::Mutex;
use tower_lsp::lsp_types::MessageType;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

struct Backend {
    client: Client,
    clangd: Mutex<Option<ClangdSession>>,
    root_uri: Mutex<Option<Url>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResult, tower_lsp::jsonrpc::Error> {
        let root_uri_json = serde_json::to_string(&params.root_uri).unwrap_or("null".to_string());
        *self.root_uri.lock().await = params.root_uri;
        match ClangdSession::new().await {
            Ok(session) => {
                let mut lock = self.clangd.lock().await;
                *lock = Some(session);
                self.client
                    .log_message(MessageType::INFO, "clangd started")
                    .await;
                if let Some(session) = lock.as_mut() {
                    let init_payload = format!(
                        r#"{{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {{"processId": null, "rootUri": {}, "capabilities": {{}}}}}}"#,
                        root_uri_json
                    );
                    let init_request = format!(
                        "Content-Length: {}\r\n\r\n{}",
                        init_payload.len(),
                        init_payload
                    );
                    if let Err(e) = session.send_notification(&init_request).await {
                        self.client
                            .log_message(
                                MessageType::ERROR,
                                format!("Failed to send initialize to clangd: {}", e),
                            )
                            .await;
                    }
                }
            }
            Err(e) => {
                self.client
                    .log_message(MessageType::ERROR, format!("Failed to start clangd: {}", e))
                    .await;
            }
        }
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "mylsp".to_string(),
                version: Some("0.1.0".to_string()),
            }),
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![".".to_string(), "::".to_string()]),
                    ..Default::default()
                }),
                semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                    legend: SemanticTokensLegend {
                        token_types: vec![
                            SemanticTokenType::NAMESPACE,
                            SemanticTokenType::TYPE,
                            SemanticTokenType::CLASS,
                            SemanticTokenType::ENUM,
                            SemanticTokenType::INTERFACE,
                            SemanticTokenType::STRUCT,
                            SemanticTokenType::TYPE_PARAMETER,
                            SemanticTokenType::PARAMETER,
                            SemanticTokenType::VARIABLE,
                            SemanticTokenType::PROPERTY,
                            SemanticTokenType::ENUM_MEMBER,
                            SemanticTokenType::EVENT,
                            SemanticTokenType::FUNCTION,
                            SemanticTokenType::METHOD,
                            SemanticTokenType::MACRO,
                            SemanticTokenType::KEYWORD,
                            SemanticTokenType::MODIFIER,
                            SemanticTokenType::COMMENT,
                            SemanticTokenType::STRING,
                            SemanticTokenType::NUMBER,
                            SemanticTokenType::REGEXP,
                            SemanticTokenType::OPERATOR,
                        ],
                        token_modifiers: vec![
                            SemanticTokenModifier::DECLARATION,
                            SemanticTokenModifier::DEFINITION,
                            SemanticTokenModifier::READONLY,
                            SemanticTokenModifier::STATIC,
                            SemanticTokenModifier::DEPRECATED,
                            SemanticTokenModifier::ABSTRACT,
                            SemanticTokenModifier::ASYNC,
                            SemanticTokenModifier::MODIFICATION,
                            SemanticTokenModifier::DOCUMENTATION,
                            SemanticTokenModifier::DEFAULT_LIBRARY,
                        ],
                    },
                    range: Some(true),
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                    ..Default::default()
                })),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "multi-lsp initialized!")
            .await;
        let mut lock = self.clangd.lock().await;
        if let Some(session) = lock.as_mut() {
            let initialized_payload =
                r#"{"jsonrpc": "2.0", "method": "initialized", "params": {}}"#;
            let request = format!(
                "Content-Length: {}\r\n\r\n{}",
                initialized_payload.len(),
                initialized_payload
            );
            if let Err(e) = session.send_notification(&request).await {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("Failed to send initialized to clangd: {}", e),
                    )
                    .await;
            }
        }
    }

    async fn shutdown(&self) -> Result<(), tower_lsp::jsonrpc::Error> {
        Ok(())
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>, tower_lsp::jsonrpc::Error> {
        let file_uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;

        self.client
            .log_message(
                MessageType::INFO,
                &format!("hover at line {}, char {}", position.line, position.character),
            )
            .await;

        let mut clangd = self.clangd.lock().await;
        let response = if let Some(session) = clangd.as_mut() {
            let hover_request = session
                .send_hover(&file_uri, position.line, position.character)
                .await;
            self.client
                .log_message(MessageType::INFO, &format!("sent hover request for {}", file_uri))
                .await;
            hover_request
        } else {
            return Ok(Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(
                    "clangd 未初始化".to_string(),
                )),
                range: None,
            }));
        };

        if response.starts_with("error:") {
            if response.contains("TimedOut") {
                return Ok(None);
            } else {
                return Ok(Some(Hover {
                    contents: HoverContents::Scalar(MarkedString::String(response)),
                    range: None,
                }));
            }
        }

        self.client
            .log_message(MessageType::INFO, &format!("clangd response: {}", response))
            .await;

        let parsed: serde_json::Value = match serde_json::from_str(&response) {
            Ok(v) => v,
            Err(e) => {
                self.client
                    .log_message(MessageType::ERROR, format!("解析 clangd 响应失败: {}", e))
                    .await;
                return Ok(Some(Hover {
                    contents: HoverContents::Scalar(MarkedString::String(
                        "解析 clangd 响应失败".to_string(),
                    )),
                    range: None,
                }));
            }
        };

        if let Some(error) = parsed.get("error") {
            return Ok(Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(format!(
                    "clangd 错误: {}",
                    error
                ))),
                range: None,
            }));
        }

        let result = match parsed.get("result") {
            Some(r) if !r.is_null() => r,
            _ => return Ok(None),
        };

        let contents = match result.get("contents") {
            Some(c) if c.is_string() => c
                .as_str()
                .map(|s| HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::PlainText,
                    value: s.to_string(),
                })),
            Some(c) if c.is_array() => c
                .as_array()
                .and_then(|arr| arr.get(0))
                .and_then(|v| v.as_str())
                .map(|s| HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::PlainText,
                    value: s.to_string(),
                })),
            Some(c) if c.is_object() => {
                if let Some(value) = c.get("value").and_then(|v| v.as_str()) {
                    Some(HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: value.to_string(),
                    }))
                } else {
                    None
                }
            }
            _ => None,
        };

        match contents {
            Some(c) => Ok(Some(Hover {
                contents: c,
                range: None,
            })),
            None => Ok(None),
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>, tower_lsp::jsonrpc::Error> {
        let file_uri = params.text_document_position.text_document.uri.to_string();
        let position = params.text_document_position.position;

        self.client
            .log_message(
                MessageType::INFO,
                &format!("completion at line {}, char {}", position.line, position.character),
            )
            .await;

        let mut clangd = self.clangd.lock().await;
        let response = if let Some(session) = clangd.as_mut() {
            let completion_request = session
                .send_completion(&file_uri, position.line, position.character)
                .await;
            self.client
                .log_message(MessageType::INFO, &format!("sent completion request for {}", file_uri))
                .await;
            completion_request
        } else {
            return Ok(None);
        };

        if response.starts_with("error:") {
            if response.contains("TimedOut") {
                return Ok(None);
            } else {
                self.client
                    .log_message(MessageType::ERROR, &format!("completion error: {}", response))
                    .await;
                return Ok(None);
            }
        }

        self.client
            .log_message(MessageType::INFO, &format!("clangd completion response: {}", response))
            .await;

        let parsed: serde_json::Value = match serde_json::from_str(&response) {
            Ok(v) => v,
            Err(e) => {
                self.client
                    .log_message(MessageType::ERROR, format!("解析 clangd completion 响应失败: {}", e))
                    .await;
                return Ok(None);
            }
        };

        if let Some(error) = parsed.get("error") {
            self.client
                .log_message(MessageType::ERROR, format!("clangd completion error: {}", error))
                .await;
            return Ok(None);
        }

        let result = match parsed.get("result") {
            Some(r) if !r.is_null() => r,
            _ => return Ok(None),
        };

        // Parse completion items
        if let Some(items) = result.get("items").and_then(|i| i.as_array()) {
            let completion_items: Vec<CompletionItem> = items
                .iter()
                .filter_map(|item| {
                    let label = item.get("label")?.as_str()?.to_string();
                    let kind = item.get("kind").and_then(|k| k.as_u64()).map(|k| match k {
                        1 => CompletionItemKind::TEXT,
                        2 => CompletionItemKind::METHOD,
                        3 => CompletionItemKind::FUNCTION,
                        4 => CompletionItemKind::CONSTRUCTOR,
                        5 => CompletionItemKind::FIELD,
                        6 => CompletionItemKind::VARIABLE,
                        7 => CompletionItemKind::CLASS,
                        8 => CompletionItemKind::INTERFACE,
                        9 => CompletionItemKind::MODULE,
                        10 => CompletionItemKind::PROPERTY,
                        11 => CompletionItemKind::UNIT,
                        12 => CompletionItemKind::VALUE,
                        13 => CompletionItemKind::ENUM,
                        14 => CompletionItemKind::KEYWORD,
                        15 => CompletionItemKind::SNIPPET,
                        16 => CompletionItemKind::COLOR,
                        17 => CompletionItemKind::FILE,
                        18 => CompletionItemKind::REFERENCE,
                        19 => CompletionItemKind::FOLDER,
                        20 => CompletionItemKind::ENUM_MEMBER,
                        21 => CompletionItemKind::CONSTANT,
                        22 => CompletionItemKind::STRUCT,
                        23 => CompletionItemKind::EVENT,
                        24 => CompletionItemKind::OPERATOR,
                        25 => CompletionItemKind::TYPE_PARAMETER,
                        _ => CompletionItemKind::TEXT,
                    });
                    let detail = item.get("detail").and_then(|d| d.as_str()).map(|s| s.to_string());
                    let documentation = item.get("documentation").and_then(|d| d.as_str()).map(|s| Documentation::String(s.to_string()));
                    let insert_text = item.get("insertText").and_then(|it| it.as_str()).map(|s| s.to_string());
                    let sort_text = item.get("sortText").and_then(|st| st.as_str()).map(|s| s.to_string());

                    Some(CompletionItem {
                        label,
                        kind,
                        detail,
                        documentation,
                        insert_text,
                        sort_text,
                        ..Default::default()
                    })
                })
                .collect();

            Ok(Some(CompletionResponse::Array(completion_items)))
        } else {
            Ok(None)
        }
    }

    async fn semantic_tokens_full(&self, params: SemanticTokensParams) -> Result<Option<SemanticTokensResult>, tower_lsp::jsonrpc::Error> {
        let file_uri = params.text_document.uri.to_string();

        self.client
            .log_message(MessageType::INFO, &format!("semantic tokens for {}", file_uri))
            .await;

        let mut clangd = self.clangd.lock().await;
        let response = if let Some(session) = clangd.as_mut() {
            let semantic_request = session
                .send_semantic_tokens(&file_uri)
                .await;
            self.client
                .log_message(MessageType::INFO, &format!("sent semantic tokens request for {}", file_uri))
                .await;
            semantic_request
        } else {
            return Ok(None);
        };

        if response.starts_with("error:") {
            if response.contains("TimedOut") {
                return Ok(None);
            } else {
                self.client
                    .log_message(MessageType::ERROR, &format!("semantic tokens error: {}", response))
                    .await;
                return Ok(None);
            }
        }

        self.client
            .log_message(MessageType::INFO, &format!("clangd semantic tokens response: {}", response))
            .await;

        let parsed: serde_json::Value = match serde_json::from_str(&response) {
            Ok(v) => v,
            Err(e) => {
                self.client
                    .log_message(MessageType::ERROR, format!("解析 clangd semantic tokens 响应失败: {}", e))
                    .await;
                return Ok(None);
            }
        };

        if let Some(error) = parsed.get("error") {
            self.client
                .log_message(MessageType::ERROR, format!("clangd semantic tokens error: {}", error))
                .await;
            return Ok(None);
        }

        let result = match parsed.get("result") {
            Some(r) if !r.is_null() => r,
            _ => return Ok(None),
        };

        if let Some(data) = result.get("data").and_then(|d| d.as_array()) {
            let tokens: Vec<SemanticToken> = data
                .chunks(5)
                .filter_map(|chunk| {
                    if chunk.len() == 5 {
                        let delta_line = chunk[0].as_u64()? as u32;
                        let delta_start = chunk[1].as_u64()? as u32;
                        let length = chunk[2].as_u64()? as u32;
                        let token_type = chunk[3].as_u64()? as u32;
                        let token_modifiers_bitset = chunk[4].as_u64()? as u32;
                        Some(SemanticToken {
                            delta_line,
                            delta_start,
                            length,
                            token_type,
                            token_modifiers_bitset,
                        })
                    } else {
                        None
                    }
                })
                .collect();

            Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: None,
                data: tokens,
            })))
        } else {
            Ok(None)
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "did_open called")
            .await;
        let mut clangd = self.clangd.lock().await;
        if let Some(session) = clangd.as_mut() {
            let payload = format!(
                r#"{{"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {{"textDocument": {{"uri": "{}", "languageId": "{}", "version": {}, "text": "{}"}}}}}}"#,
                params.text_document.uri,
                params.text_document.language_id,
                params.text_document.version,
                params
                    .text_document
                    .text
                    .replace("\\", "\\\\")
                    .replace("\"", "\\\"")
                    .replace("\n", "\\n")
                    .replace("\r", "\\r")
            );
            let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);
            self.client
                .log_message(MessageType::INFO, &format!("sending didOpen: {}", request))
                .await;
            if let Err(e) = session.send_notification(&request).await {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("Failed to send didOpen to clangd: {}", e),
                    )
                    .await;
            }
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let mut clangd = self.clangd.lock().await;
        if let Some(session) = clangd.as_mut() {
            let payload = format!(
                r#"{{"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {{"textDocument": {{"uri": "{}", "version": {}}}, "contentChanges": {}}}}}"#,
                params.text_document.uri,
                params.text_document.version,
                serde_json::to_string(&params.content_changes).unwrap()
            );
            let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);
            if let Err(e) = session.send_notification(&request).await {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("Failed to send didChange to clangd: {}", e),
                    )
                    .await;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let mut clangd = self.clangd.lock().await;
        if let Some(session) = clangd.as_mut() {
            let payload = format!(
                r#"{{"jsonrpc": "2.0", "method": "textDocument/didClose", "params": {{"textDocument": {{"uri": "{}"}}}}}}"#,
                params.text_document.uri
            );
            let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);
            if let Err(e) = session.send_notification(&request).await {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("Failed to send didClose to clangd: {}", e),
                    )
                    .await;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend {
        client,
        clangd: Mutex::new(None),
        root_uri: Mutex::new(None),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
