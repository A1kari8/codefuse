//! response_parser.rs - LSP 响应解析模块
//!
//! 这个模块负责解析来自语言服务器的 JSON 响应，
//! 将其转换为相应的 LSP 类型结构。
//!
//! # 主要功能
//!
//! - 统一的错误处理逻辑
//! - 各种 LSP 方法响应的解析
//! - 类型安全的数据转换
//!
//! # 支持的响应类型
//!
//! - Hover: 悬停信息
//! - Completion: 代码补全
//! - SemanticTokens: 语义令牌
//! - DocumentHighlight: 文档高亮
//! - FoldingRange: 折叠范围
//! - WorkspaceEdit: 重命名编辑

use serde_json;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, jsonrpc::Error as JsonRpcError};

/// 通用的 LSP 响应解析结果
pub type ParseResult<T> = Result<Option<T>, JsonRpcError>;

/// 解析 clangd 响应的通用错误处理
///
/// 这个函数处理常见的错误情况：
/// - 超时错误
/// - JSON 解析错误
/// - clangd 返回的错误
///
/// # Arguments
///
/// * `response` - 原始响应字符串
/// * `client` - LSP 客户端，用于日志记录
/// * `method_name` - 方法名称，用于错误消息
///
/// # Returns
///
/// 返回解析后的 JSON 值，如果出错则返回 None
async fn parse_clangd_response(
    response: &str,
    client: &Client,
    method_name: &str,
) -> Option<serde_json::Value> {
    // 检查是否为错误响应
    if response.starts_with("error:") {
        if response.contains("TimedOut") {
            return None;
        } else {
            client
                .log_message(
                    MessageType::ERROR,
                    &format!("{} error: {}", method_name, response),
                )
                .await;
            return None;
        }
    }

    // 解析 JSON 响应
    let parsed: serde_json::Value = match serde_json::from_str(response) {
        Ok(v) => v,
        Err(e) => {
            client
                .log_message(
                    MessageType::ERROR,
                    format!("解析 clangd {} 响应失败: {}", method_name, e),
                )
                .await;
            return None;
        }
    };

    // 检查是否有错误
    if let Some(error) = parsed.get("error") {
        client
            .log_message(
                MessageType::ERROR,
                format!("clangd {} error: {}", method_name, error),
            )
            .await;
        return None;
    }

    Some(parsed)
}

/// 解析悬停响应
///
/// 从 LSP 服务器的 JSON 响应中提取悬停信息
pub async fn parse_hover_response(
    response: &str,
    client: &Client,
) -> ParseResult<Hover> {
    // 处理错误响应
    if response.starts_with("error:") {
        if response.contains("TimedOut") {
            return Ok(None);
        } else {
            return Ok(Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(response.to_string())),
                range: None,
            }));
        }
    }

    // 解析 JSON 响应
    let parsed: serde_json::Value = match serde_json::from_str(response) {
        Ok(v) => v,
        Err(e) => {
            client
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

    // 检查是否有错误字段
    if let Some(error) = parsed.get("error") {
        return Ok(Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String(format!(
                "clangd 错误: {}",
                error
            ))),
            range: None,
        }));
    }

    // 提取结果内容
    if let Some(result) = parsed.get("result") {
        if let Some(contents) = result.get("contents") {
            let hover_contents = parse_hover_contents(contents);
            let range = result
                .get("range")
                .and_then(|r| serde_json::from_value(r.clone()).ok());

            return Ok(Some(Hover {
                contents: hover_contents,
                range,
            }));
        }
    }

    // 没有找到有效内容
    Ok(None)
}

/// 解析 hover 内容的具体格式
///
/// LSP hover 响应中的 contents 字段可以有多种格式：
/// - 字符串
/// - 数组（取第一个元素）
/// - 对象（提取 value 字段）
fn parse_hover_contents(contents: &serde_json::Value) -> HoverContents {
    match contents {
        // 字符串格式
        serde_json::Value::String(s) => {
            HoverContents::Scalar(MarkedString::String(s.clone()))
        }
        // 数组格式，取第一个元素
        serde_json::Value::Array(arr) if !arr.is_empty() => {
            if let Some(first) = arr.first() {
                parse_hover_contents(first)
            } else {
                HoverContents::Scalar(MarkedString::String("".to_string()))
            }
        }
        // 对象格式，尝试提取 value 字段
        serde_json::Value::Object(obj) => {
            if let Some(value) = obj.get("value") {
                if let Some(s) = value.as_str() {
                    HoverContents::Scalar(MarkedString::String(s.to_string()))
                } else {
                    HoverContents::Scalar(MarkedString::String(format!("{:?}", value)))
                }
            } else {
                HoverContents::Scalar(MarkedString::String(format!("{:?}", contents)))
            }
        }
        // 其他格式，转为字符串
        _ => HoverContents::Scalar(MarkedString::String(format!("{:?}", contents))),
    }
}

/// 解析代码补全响应
///
/// 从 LSP 服务器的 JSON 响应中提取代码补全项
pub async fn parse_completion_response(
    response: &str,
    client: &Client,
) -> ParseResult<CompletionResponse> {
    let parsed = match parse_clangd_response(response, client, "completion").await {
        Some(p) => p,
        None => return Ok(None),
    };

    // 提取结果
    let result = match parsed.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return Ok(None),
    };

    // 解析补全项
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

/// 解析语义令牌响应
///
/// 从 LSP 服务器的 JSON 响应中提取语义令牌数据
pub async fn parse_semantic_tokens_response(
    response: &str,
    client: &Client,
) -> ParseResult<SemanticTokensResult> {
    let parsed = match parse_clangd_response(response, client, "semantic tokens").await {
        Some(p) => p,
        None => return Ok(None),
    };

    // 提取结果
    let result = match parsed.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return Ok(None),
    };

    // 解析语义令牌数据
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

/// 解析文档高亮响应
///
/// 从 LSP 服务器的 JSON 响应中提取文档高亮信息
pub async fn parse_document_highlight_response(
    response: &str,
    client: &Client,
) -> ParseResult<Vec<DocumentHighlight>> {
    let parsed = match parse_clangd_response(response, client, "document highlight").await {
        Some(p) => p,
        None => return Ok(None),
    };

    // 提取结果
    let result = match parsed.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return Ok(None),
    };

    // 解析文档高亮数组
    if let Some(highlights) = result.as_array() {
        let document_highlights: Vec<DocumentHighlight> = highlights
            .iter()
            .filter_map(|highlight| {
                let range = highlight.get("range").and_then(|r| serde_json::from_value(r.clone()).ok())?;
                let kind = highlight.get("kind").and_then(|k| k.as_u64()).map(|k| match k {
                    1 => DocumentHighlightKind::TEXT,
                    2 => DocumentHighlightKind::READ,
                    3 => DocumentHighlightKind::WRITE,
                    _ => DocumentHighlightKind::TEXT,
                });

                Some(DocumentHighlight { range, kind })
            })
            .collect();

        Ok(Some(document_highlights))
    } else {
        Ok(None)
    }
}

/// 解析折叠范围响应
///
/// 从 LSP 服务器的 JSON 响应中提取折叠范围信息
pub async fn parse_folding_range_response(
    response: &str,
    client: &Client,
) -> ParseResult<Vec<FoldingRange>> {
    let parsed = match parse_clangd_response(response, client, "folding range").await {
        Some(p) => p,
        None => return Ok(None),
    };

    // 提取结果
    let result = match parsed.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return Ok(None),
    };

    // 解析折叠范围数组
    if let Some(ranges) = result.as_array() {
        let folding_ranges: Vec<FoldingRange> = ranges
            .iter()
            .filter_map(|range| {
                let start_line = range.get("startLine")?.as_u64()? as u32;
                let end_line = range.get("endLine")?.as_u64()? as u32;
                let start_character = range.get("startCharacter").and_then(|c| c.as_u64()).map(|c| c as u32);
                let end_character = range.get("endCharacter").and_then(|c| c.as_u64()).map(|c| c as u32);
                let kind = range.get("kind").and_then(|k| k.as_str()).map(|k| match k {
                    "comment" => FoldingRangeKind::Comment,
                    "imports" => FoldingRangeKind::Imports,
                    "region" => FoldingRangeKind::Region,
                    _ => FoldingRangeKind::Region,
                });

                Some(FoldingRange {
                    start_line,
                    end_line,
                    start_character,
                    end_character,
                    kind,
                    collapsed_text: None,
                })
            })
            .collect();

        Ok(Some(folding_ranges))
    } else {
        Ok(None)
    }
}

/// 解析重命名响应
///
/// 从 LSP 服务器的 JSON 响应中提取重命名编辑信息
pub async fn parse_rename_response(
    response: &str,
    client: &Client,
) -> ParseResult<WorkspaceEdit> {
    let parsed = match parse_clangd_response(response, client, "rename").await {
        Some(p) => p,
        None => return Ok(None),
    };

    // 提取结果
    let result = match parsed.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return Ok(None),
    };

    // 解析 WorkspaceEdit
    match serde_json::from_value(result.clone()) {
        Ok(workspace_edit) => Ok(Some(workspace_edit)),
        Err(e) => {
            client
                .log_message(
                    MessageType::ERROR,
                    format!("解析 WorkspaceEdit 失败: {}", e),
                )
                .await;
            Ok(None)
        }
    }
}