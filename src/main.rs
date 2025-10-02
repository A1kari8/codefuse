//! main.rs - 多语言 LSP 服务器主程序
//!
//! 这个模块实现了一个支持多种编程语言的 LSP (语言服务器协议) 服务器。
//! 它作为一个代理层，根据初始化选项选择合适的语言服务器后端。
//!
//! # 支持的语言服务器
//!
//! - **clangd**: 用于 C/C++ 语言支持
//! - **MockLspServer**: 用于测试和开发的模拟服务器
//!
//! # 架构
//!
//! - `Backend`: 主要的 LSP 服务器后端，实现 tower_lsp::LanguageServer trait
//! - `create_lsp_server()`: 根据语言类型创建对应的语言服务器实例
//! - LSP 方法实现：悬停信息、代码补全、语义令牌等
//!
//! # 使用方式
//!
//! 服务器通过标准输入/输出进行通信，客户端可以在初始化时
//! 指定 `language` 参数来选择要使用的语言服务器。

// 内部模块定义
mod clangd;           // clangd 语言服务器实现
mod lsp_server;       // LSP 服务器的通用 trait 定义
mod mock_lsp_server;  // 模拟 LSP 服务器实现

// 内部模块导入
use crate::clangd::ClangdSession;
use crate::lsp_server::LspServer;
use crate::mock_lsp_server::MockLspServer;

// 外部库导入
use serde_json;                                   // JSON 序列化/反序列化
use tokio::sync::Mutex;                           // 异步互斥锁
use tower_lsp::lsp_types::MessageType;            // LSP 消息类型
use tower_lsp::lsp_types::*;                      // LSP 类型定义
use tower_lsp::{Client, LanguageServer, LspService, Server}; // tower-lsp 框架

/// 根据语言类型创建相应的 LSP 服务器实例
///
/// 这个函数是一个工厂函数，根据传入的语言标识符创建对应的
/// 语言服务器实例。所有返回的实例都实现了 `LspServer` trait。
///
/// # Arguments
///
/// * `language` - 语言标识符字符串，支持的值：
///   - "cpp" 或 "c": 创建 clangd 服务器实例
///   - "mock": 创建模拟服务器实例（用于测试）
///
/// # Returns
///
/// 成功时返回 `Ok(Box<dyn LspServer>)`，包含创建的服务器实例
/// 失败时返回 `Err(std::io::Error)`
///
/// # Errors
///
/// 在以下情况下会返回错误：
/// - 传入不支持的语言标识符
/// - clangd 服务器初始化失败（如 clangd 未安装）
///
/// # 示例
///
/// ```ignore
/// let cpp_server = create_lsp_server("cpp").await?;
/// let mock_server = create_lsp_server("mock").await?;
/// ```
async fn create_lsp_server(language: &str) -> Result<Box<dyn LspServer>, std::io::Error> {
    match language {
        "cpp" | "c" => {
            let session = ClangdSession::new().await?;
            Ok(Box::new(session))
        }
        "mock" => Ok(Box::new(MockLspServer::new())),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Unsupported language: {}", language),
        )),
    }
}

/// Backend 结构体 - 主要的 LSP 服务器后端实现
///
/// 这个结构体实现了 tower_lsp::LanguageServer trait，提供所有 LSP 方法的实现。
/// 它作为一个代理层，将 LSP 请求转发给具体的语言服务器后端。
///
/// # 字段
///
/// * `client` - tower-lsp 客户端，用于向客户端发送消息和日志
/// * `lsp_server` - 当前活跃的语言服务器实例，使用 Mutex 保证线程安全
/// * `root_uri` - 工作区的根 URI，用于路径解析
///
/// # 线程安全
///
/// 所有字段都使用 tokio::sync::Mutex 进行保护，支持多客户端并发访问。
///
/// # 生命周期
///
/// 1. 初始化：根据客户端参数创建对应的语言服务器
/// 2. 运行：处理各种 LSP 请求并转发给后端服务器
/// 3. 关闭：清理资源并关闭后端服务器
struct Backend {
    client: Client,
    lsp_server: Mutex<Option<Box<dyn LspServer>>>,
    root_uri: Mutex<Option<Url>>,
}

/// Backend 结构体的辅助方法实现
impl Backend {
    /// 根据范围过滤语义令牌
    ///
    /// 语义令牌使用相对位置编码，需要解码为绝对位置后进行范围过滤
    ///
    /// # Arguments
    ///
    /// * `tokens` - 完整的语义令牌数组
    /// * `range` - 需要过滤的范围
    ///
    /// # Returns
    ///
    /// 返回指定范围内的语义令牌数组
    fn filter_tokens_by_range(&self, tokens: &[SemanticToken], range: Range) -> Vec<SemanticToken> {
        let mut filtered_tokens = Vec::new();
        let mut current_line = 0u32;
        let mut current_char = 0u32;

        for token in tokens {
            // 计算令牌的绝对位置
            if token.delta_line > 0 {
                current_line += token.delta_line;
                current_char = token.delta_start;
            } else {
                current_char += token.delta_start;
            }

            let token_end_char = current_char + token.length;
            
            // 检查令牌是否在指定范围内
            let token_in_range = 
                // 令牌开始位置在范围内
                (current_line > range.start.line || 
                 (current_line == range.start.line && current_char >= range.start.character)) &&
                // 令牌结束位置在范围内  
                (current_line < range.end.line ||
                 (current_line == range.end.line && token_end_char <= range.end.character));

            if token_in_range {
                filtered_tokens.push(*token);
            }
        }

        // 重新计算相对位置编码
        self.recalculate_relative_positions(filtered_tokens)
    }

    /// 重新计算语义令牌的相对位置编码
    ///
    /// 由于过滤后的令牌需要重新计算相对位置，这个方法将绝对位置转换回相对位置
    fn recalculate_relative_positions(&self, mut tokens: Vec<SemanticToken>) -> Vec<SemanticToken> {
        if tokens.is_empty() {
            return tokens;
        }

        let mut prev_line = 0u32;
        let mut prev_char = 0u32;
        let mut current_line = 0u32;
        let mut current_char = 0u32;

        for token in &mut tokens {
            // 计算当前令牌的绝对位置
            if token.delta_line > 0 {
                current_line += token.delta_line;
                current_char = token.delta_start;
            } else {
                current_char += token.delta_start;
            }

            // 重新计算相对位置
            let new_delta_line = current_line - prev_line;
            let new_delta_start = if new_delta_line > 0 {
                current_char
            } else {
                current_char - prev_char
            };

            token.delta_line = new_delta_line;
            token.delta_start = new_delta_start;

            // 更新前一个位置
            prev_line = current_line;
            prev_char = current_char;
        }

        tokens
    }
}

/// Backend 的 LanguageServer trait 实现
/// 
/// 实现了完整的 LSP 协议方法，包括初始化、悬停信息、代码补全、
/// 语义令牌、文档生命周期管理等。
#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    /// LSP 服务器初始化方法
    ///
    /// 这是 LSP 握手过程的第一步，客户端发送初始化参数，服务器：
    /// 1. 解析客户端传递的语言类型参数
    /// 2. 创建对应的语言服务器后端实例
    /// 3. 向后端发送初始化请求
    /// 4. 返回服务器能力声明
    ///
    /// # Arguments
    ///
    /// * `params` - 客户端发送的初始化参数，包含工作区信息和初始化选项
    ///
    /// # Returns
    ///
    /// 返回 `InitializeResult`，包含服务器信息和支持的 LSP 能力
    ///
    /// # 支持的初始化选项
    ///
    /// - `language`: 指定要使用的语言服务器类型 ("cpp", "c", "mock")
    ///
    /// # 服务器能力
    ///
    /// 返回的能力包括：
    /// - 悬停信息提供
    /// - 增量文档同步
    /// - 代码补全（触发字符：".", "::"）
    /// - 语义令牌（用于语法高亮）
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResult, tower_lsp::jsonrpc::Error> {
        let root_uri_json = serde_json::to_string(&params.root_uri).unwrap_or("null".to_string());
        *self.root_uri.lock().await = params.root_uri;

        // Determine which LSP server to use based on initialization options
        let language = if let Some(opts) = &params.initialization_options {
            if let Some(lang) = opts.get("language").and_then(|v| v.as_str()) {
                lang
            } else {
                "cpp" // default to C++
            }
        } else {
            "cpp" // default to C++
        };

        self.client
            .log_message(
                MessageType::INFO,
                &format!("Using LSP server for language: {}", language),
            )
            .await;

        match create_lsp_server(language).await {
            Ok(server) => {
                let mut lock = self.lsp_server.lock().await;
                *lock = Some(server);
                self.client
                    .log_message(
                        MessageType::INFO,
                        &format!("{} LSP server started", language),
                    )
                    .await;
                if let Some(session) = lock.as_mut() {
                    let init_payload = format!(
                        r#"{{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {{"processId": {}, "rootUri": {}, "capabilities": {} }}}}"#,
                        serde_json::to_string(&params.process_id).unwrap_or("null".to_string()),
                        root_uri_json,
                        serde_json::to_string(&params.capabilities).unwrap_or("{}".to_string())
                    );
                    let init_request = format!(
                        "Content-Length: {}\r\n\r\n{}",
                        init_payload.len(),
                        init_payload
                    );
                    match session.send_request(&init_request).await {
                        Ok(response) => {
                            // 解析后端响应
                            let parsed: serde_json::Value = serde_json::from_str(&response).unwrap_or_default();
                            let backend_capabilities = parsed.get("result").and_then(|r| r.get("capabilities")).cloned().unwrap_or(serde_json::json!({}));
                            
                            // 尝试从后端能力构建代理能力
                            let server_capabilities = serde_json::from_value(backend_capabilities).unwrap_or_else(|_| ServerCapabilities {
                                hover_provider: Some(HoverProviderCapability::Simple(true)),
                                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                                    TextDocumentSyncKind::INCREMENTAL,
                                )),
                                completion_provider: Some(CompletionOptions {
                                    resolve_provider: Some(false),
                                    trigger_characters: Some(vec![".".to_string(), "::".to_string()]),
                                    ..Default::default()
                                }),
                                semantic_tokens_provider: Some(
                                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                                        SemanticTokensOptions {
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
                                            range: Some(true),   // 启用范围语义令牌
                                            full: Some(SemanticTokensFullOptions::Bool(true)),
                                            ..Default::default()
                                        },
                                    ),
                                ),
                                ..Default::default()
                            });
                            
                            return Ok(InitializeResult {
                                server_info: Some(ServerInfo {
                                    name: "codefuse".to_string(),
                                    version: Some("0.1.0".to_string()),
                                }),
                                capabilities: server_capabilities,
                            });
                        }
                        Err(e) => {
                            self.client
                                .log_message(
                                    MessageType::ERROR,
                                    format!("Failed to send initialize to {}: {}", language, e),
                                )
                                .await;
                        }
                    }
                }
            }
            Err(e) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("Failed to start {} LSP server: {}", language, e),
                    )
                    .await;
            }
        }
        
        // 默认返回，如果后端失败或没有响应
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "codefuse".to_string(),
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
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
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
                            range: Some(true),   // 启用范围语义令牌
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..Default::default()
                        },
                    ),
                ),
                ..Default::default()
            },
        })
    }

    /// LSP 初始化完成通知
    ///
    /// 在 initialize 方法完成后，客户端会发送此通知表示初始化过程结束。
    /// 服务器可以在此时执行一些初始化后的设置工作。
    ///
    /// # Arguments
    ///
    /// * `_` - 初始化完成参数（当前未使用）
    ///
    /// # 行为
    ///
    /// 1. 记录初始化完成日志
    /// 2. 向后端语言服务器发送 initialized 通知
    /// 3. 确保后端服务器进入可工作状态
    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "multi-lsp initialized!")
            .await;
        let mut lock = self.lsp_server.lock().await;
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

    /// LSP 服务器关闭方法
    ///
    /// 客户端请求关闭服务器时调用。这个方法应该清理资源但不能退出进程，
    /// 进程退出由后续的 exit 通知触发。
    ///
    /// # Returns
    ///
    /// 返回 `Ok(())` 表示关闭请求已接受
    ///
    /// # 注意
    ///
    /// 当前实现较简单，实际应用中可能需要：
    /// - 清理后端语言服务器连接
    /// - 保存必要的状态信息
    /// - 等待正在进行的请求完成
    async fn shutdown(&self) -> Result<(), tower_lsp::jsonrpc::Error> {
        Ok(())
    }

    /// 处理悬停信息请求
    ///
    /// 当用户将鼠标悬停在代码符号上时，客户端会发送此请求来获取符号的详细信息。
    /// 本方法将请求转发给后端语言服务器，并解析返回的响应。
    ///
    /// # Arguments
    ///
    /// * `params` - 悬停请求参数，包含文档位置信息
    ///   - `text_document_position_params.text_document.uri`: 文档 URI
    ///   - `text_document_position_params.position`: 光标位置（行号和字符位置）
    ///
    /// # Returns
    ///
    /// 返回 `Option<Hover>`：
    /// - `Some(Hover)`: 包含悬停信息的响应
    /// - `None`: 在指定位置没有悬停信息
    ///
    /// # 错误处理
    ///
    /// 本方法处理多种错误情况：
    /// - 后端语言服务器未初始化：返回提示信息
    /// - 请求超时：返回 None
    /// - JSON 解析失败：返回错误信息
    /// - clangd 返回错误：显示具体错误信息
    ///
    /// # 响应格式解析
    ///
    /// 支持多种 clangd 响应格式：
    /// - 字符串格式的内容
    /// - 数组格式的内容（取第一个元素）
    /// - 对象格式的内容（提取 value 字段）
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
                &format!(
                    "hover at line {}, char {}",
                    position.line, position.character
                ),
            )
            .await;

        let mut clangd = self.lsp_server.lock().await;
        let response = if let Some(session) = clangd.as_mut() {
            let hover_request = session
                .send_hover(&file_uri, position.line, position.character)
                .await;
            self.client
                .log_message(
                    MessageType::INFO,
                    &format!("sent hover request for {}", file_uri),
                )
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
            Some(c) if c.is_string() => c.as_str().map(|s| {
                HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::PlainText,
                    value: s.to_string(),
                })
            }),
            Some(c) if c.is_array() => c
                .as_array()
                .and_then(|arr| arr.get(0))
                .and_then(|v| v.as_str())
                .map(|s| {
                    HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::PlainText,
                        value: s.to_string(),
                    })
                }),
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

    /// 处理代码补全请求
    ///
    /// 当用户输入代码时，客户端会发送此请求来获取可能的代码补全建议。
    /// 本方法将请求转发给后端语言服务器，解析响应并返回补全项列表。
    ///
    /// # Arguments
    ///
    /// * `params` - 代码补全请求参数
    ///   - `text_document_position.text_document.uri`: 文档 URI
    ///   - `text_document_position.position`: 光标位置
    ///   - `context`: 补全上下文（触发字符、触发原因等）
    ///
    /// # Returns
    ///
    /// 返回 `Option<CompletionResponse>`：
    /// - `Some(CompletionResponse::Array)`: 包含补全项数组
    /// - `None`: 没有可用的补全建议
    ///
    /// # 补全项类型
    ///
    /// clangd 可能返回的补全类型包括：
    /// - 函数名和方法名
    /// - 变量名和字段名  
    /// - 类型名和命名空间
    /// - 宏定义
    /// - 关键字
    ///
    /// # 触发条件
    ///
    /// 本服务器配置的触发字符：
    /// - `.`: 成员访问
    /// - `::`: 命名空间/作用域解析
    ///
    /// # 错误处理
    ///
    /// - 后端服务器未初始化：返回 None
    /// - 请求超时：返回 None
    /// - JSON 解析失败：记录错误并返回 None
    /// - clangd 错误：记录错误信息并返回 None
    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>, tower_lsp::jsonrpc::Error> {
        let file_uri = params.text_document_position.text_document.uri.to_string();
        let position = params.text_document_position.position;

        self.client
            .log_message(
                MessageType::INFO,
                &format!(
                    "completion at line {}, char {}",
                    position.line, position.character
                ),
            )
            .await;

        let mut clangd = self.lsp_server.lock().await;
        let response = if let Some(session) = clangd.as_mut() {
            let completion_request = session
                .send_completion(&file_uri, position.line, position.character)
                .await;
            self.client
                .log_message(
                    MessageType::INFO,
                    &format!("sent completion request for {}", file_uri),
                )
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
                    .log_message(
                        MessageType::ERROR,
                        &format!("completion error: {}", response),
                    )
                    .await;
                return Ok(None);
            }
        }

        self.client
            .log_message(
                MessageType::INFO,
                &format!("clangd completion response: {}", response),
            )
            .await;

        let parsed: serde_json::Value = match serde_json::from_str(&response) {
            Ok(v) => v,
            Err(e) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("解析 clangd completion 响应失败: {}", e),
                    )
                    .await;
                return Ok(None);
            }
        };

        if let Some(error) = parsed.get("error") {
            self.client
                .log_message(
                    MessageType::ERROR,
                    format!("clangd completion error: {}", error),
                )
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
                    let detail = item
                        .get("detail")
                        .and_then(|d| d.as_str())
                        .map(|s| s.to_string());
                    let documentation = item
                        .get("documentation")
                        .and_then(|d| d.as_str())
                        .map(|s| Documentation::String(s.to_string()));
                    let insert_text = item
                        .get("insertText")
                        .and_then(|it| it.as_str())
                        .map(|s| s.to_string());
                    let sort_text = item
                        .get("sortText")
                        .and_then(|st| st.as_str())
                        .map(|s| s.to_string());

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

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>, tower_lsp::jsonrpc::Error> {
        let file_uri = params.text_document.uri.to_string();

        self.client
            .log_message(
                MessageType::INFO,
                &format!("semantic tokens for {}", file_uri),
            )
            .await;

        let mut clangd = self.lsp_server.lock().await;
        let response = if let Some(session) = clangd.as_mut() {
            let semantic_request = session.send_semantic_tokens(&file_uri).await;
            self.client
                .log_message(
                    MessageType::INFO,
                    &format!("sent semantic tokens request for {}", file_uri),
                )
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
                    .log_message(
                        MessageType::ERROR,
                        &format!("semantic tokens error: {}", response),
                    )
                    .await;
                return Ok(None);
            }
        }

        self.client
            .log_message(
                MessageType::INFO,
                &format!("clangd semantic tokens response: {}", response),
            )
            .await;

        let parsed: serde_json::Value = match serde_json::from_str(&response) {
            Ok(v) => v,
            Err(e) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("解析 clangd semantic tokens 响应失败: {}", e),
                    )
                    .await;
                return Ok(None);
            }
        };

        if let Some(error) = parsed.get("error") {
            self.client
                .log_message(
                    MessageType::ERROR,
                    format!("clangd semantic tokens error: {}", error),
                )
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

    /// 获取指定范围的语义令牌
    ///
    /// 当客户端请求文档某个范围的语义令牌时调用。对于 clangd，
    /// 我们通常获取整个文档的语义令牌，然后过滤出指定范围内的令牌。
    ///
    /// # Arguments
    ///
    /// * `params` - 语义令牌范围请求参数
    ///   - `text_document`: 目标文档标识
    ///   - `range`: 请求的文档范围
    ///
    /// # Returns
    ///
    /// 返回指定范围内的语义令牌，如果无法获取则返回 None
    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>, tower_lsp::jsonrpc::Error> {
        let file_uri = params.text_document.uri.to_string();
        let range = params.range;

        self.client
            .log_message(
                MessageType::INFO,
                &format!(
                    "semantic tokens range for {} ({}:{} to {}:{})",
                    file_uri,
                    range.start.line,
                    range.start.character,
                    range.end.line,
                    range.end.character
                ),
            )
            .await;

        // 对于 clangd，我们获取完整的语义令牌，然后过滤范围
        // 这是因为 clangd 通常提供整个文档的语义令牌
        let full_params = SemanticTokensParams {
            text_document: params.text_document,
            work_done_progress_params: params.work_done_progress_params,
            partial_result_params: params.partial_result_params,
        };

        match self.semantic_tokens_full(full_params).await? {
            Some(SemanticTokensResult::Tokens(full_tokens)) => {
                // 过滤出指定范围内的令牌
                let filtered_tokens = self.filter_tokens_by_range(&full_tokens.data, range);
                
                Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
                    result_id: full_tokens.result_id,
                    data: filtered_tokens,
                })))
            }
            Some(SemanticTokensResult::Partial(_)) => {
                // 处理部分结果的情况（通常不会在我们的实现中出现）
                Ok(None)
            }
            None => Ok(None),
        }
    }

    /// 处理文档打开通知
    ///
    /// 当客户端打开一个文档时调用。这个通知会将文档的内容发送给后端语言服务器，
    /// 使其能够建立语义模型并提供语言服务。
    ///
    /// # Arguments
    ///
    /// * `params` - 文档打开参数
    ///   - `text_document.uri`: 文档 URI
    ///   - `text_document.language_id`: 语言标识符（如 "cpp", "c"）
    ///   - `text_document.version`: 文档版本号
    ///   - `text_document.text`: 文档完整内容
    ///
    /// # 行为
    ///
    /// 1. 记录文档打开日志
    /// 2. 构造 LSP textDocument/didOpen 通知
    /// 3. 将文档内容转义处理（转义特殊字符）
    /// 4. 向后端语言服务器发送通知
    ///
    /// # 注意
    ///
    /// 文档内容中的特殊字符（如换行符、引号等）会被转义以确保 JSON 格式正确。
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.client
            .log_message(MessageType::INFO, "did_open called")
            .await;
        let mut clangd = self.lsp_server.lock().await;
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

    /// 处理文档内容变更通知
    ///
    /// 当客戸端修改文档内容时调用。这个通知包含增量更新信息，
    /// 后端语言服务器可以根据这些变更更新其内部的语义模型。
    ///
    /// # Arguments
    ///
    /// * `params` - 文档变更参数
    ///   - `text_document.uri`: 文档 URI
    ///   - `text_document.version`: 新的文档版本号
    ///   - `content_changes`: 变更列表，包含具体的修改内容
    ///
    /// # 同步模式
    ///
    /// 本服务器配置为支持增量同步 (INCREMENTAL)，只传送变更的部分而非整个文档。
    ///
    /// # 性能考虑
    ///
    /// 增量更新减少了网络传输量和后端处理开销，特别是对于大文件。
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let mut clangd = self.lsp_server.lock().await;
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

    /// 处理文档关闭通知
    ///
    /// 当客户端关闭一个文档时调用。后端语言服务器会清理与该文档相关的
    /// 内部状态和资源，释放内存。
    ///
    /// # Arguments
    ///
    /// * `params` - 文档关闭参数
    ///   - `text_document.uri`: 要关闭的文档 URI
    ///
    /// # 行为
    ///
    /// 1. 构造 LSP textDocument/didClose 通知
    /// 2. 向后端语言服务器发送通知
    /// 3. 后端服务器清理文档相关资源
    ///
    /// # 资源管理
    ///
    /// 对于大型项目，适时关闭不再使用的文档可以显著减少内存使用量。
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let mut clangd = self.lsp_server.lock().await;
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

/// 程序入口点 - 启动多语言 LSP 服务器
///
/// 这个函数初始化并启动 LSP 服务器，它：
/// 1. 设置异步运行时（通过 #[tokio::main]）
/// 2. 创建标准输入/输出流用于与客户端通信
/// 3. 创建 Backend 实例和 LspService
/// 4. 启动服务器并开始处理客户端请求
///
/// # LSP 通信协议
///
/// LSP 服务器通过标准输入/输出与客户端进行通信：
/// - stdin: 接收来自客户端的请求和通知
/// - stdout: 发送响应和通知给客户端
/// - stderr: 用于日志输出（不会干扰 LSP 协议）
///
/// # 服务器架构
///
/// ```text
/// 客户端 (VS Code, etc.)
///     ↕ (stdin/stdout, JSON-RPC over LSP)
/// Backend (本程序)
///     ↕ (进程通信)
/// 后端语言服务器 (clangd, etc.)
/// ```
///
/// # 并发处理
///
/// 服务器可以并发处理多个客户端请求，每个请求在独立的异步任务中执行。
/// 通过 Mutex 保护共享状态确保线程安全。
///
/// # 生命周期
///
/// 服务器会一直运行直到：
/// - 客户端发送 shutdown 请求后跟随 exit 通知
/// - 进程收到终止信号
/// - 发生不可恢复的错误
#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend {
        client,
        lsp_server: Mutex::new(None),
        root_uri: Mutex::new(None),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
