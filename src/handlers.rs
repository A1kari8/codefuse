use std::sync::Arc;
use serde_json;
use tower_lsp::lsp_types::{InitializeResult, ServerInfo};
use tokio::sync::mpsc;
use anyhow;
use futures::future::BoxFuture;

use crate::dispatcher::Dispatcher;

/// 处理 initialize 请求的处理器。
///
/// 这个函数修改 clangd 的初始化响应，设置服务器信息。
///
/// # 参数
///
/// * `rpc` - 接收到的 RPC 消息
/// * `frontend_sender` - 发送消息到前端的通道
///
/// # 返回
///
/// 返回 `BoxFuture` 包装的 `Result<()>`，表示处理是否成功
fn handle_initialize(
    rpc: serde_json::Value,
    frontend_sender: mpsc::UnboundedSender<String>,
) -> BoxFuture<'static, anyhow::Result<()>> {
    Box::pin(async move {
        let mut raw_rpc = rpc.clone();
        // Step 1: 转成 tower-lsp
        let raw_result = rpc
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Missing result field"))?;
        let mut init_result: InitializeResult = serde_json::from_value(raw_result)?;

        init_result.server_info = Some(ServerInfo {
            name: "codefuse".into(),
            version: Some("0.1.0".into()),
        });

        let edited = serde_json::to_value(init_result)?;

        if let Some(obj) = raw_rpc.as_object_mut() {
            obj.insert("result".to_string(), edited); // 修改字段
        }

        // Step 3: 转回 JSON
        let message = Dispatcher::format_lsp_message(&raw_rpc)?;
        frontend_sender.send(message)?;
        Ok(())
    })
}

/// 设置处理器函数，为特定的 LSP 方法注册处理逻辑。
///
/// 这个函数用于注册从后端（clangd）接收到的消息的处理函数。
/// 目前注册了 `initialize` 方法的处理器，用于修改初始化响应。
///
/// # 参数
///
/// * `dispatcher` - 调度器实例，用于注册处理器
///
/// # 示例
///
/// ```rust
/// setup_handlers(dispatcher.clone()).await;
/// ```
pub async fn setup_handlers(dispatcher: Arc<Dispatcher>) {
    // reg.register("textDocument/hover", move |params,clangd,vscode_out| {
    //     async move {
    //         let mut client = clangd.lock().await;
    //         let raw_result = client.send_request("textDocument/hover", params).await?;
    //
    //         // Step 1: 转成 Hover 类型
    //         let mut hover: Hover = serde_json::from_value(raw_result).ok()?;
    //
    //         // Step 2: 编辑 Hover 内容
    //         match &mut hover.contents {
    //             HoverContents::Scalar(MarkedString::String(s)) => {
    //                 s.push_str("\n\n---\nEnhanced by proxy");
    //             }
    //             HoverContents::Scalar(MarkedString::LanguageString(ls)) => {
    //                 ls.value.push_str("\n\n// Enhanced by proxy");
    //             }
    //             HoverContents::Array(arr) => {
    //                 arr.push(MarkedString::String("Enhanced by proxy".into()));
    //             }
    //             _ => {}
    //         }
    //
    //         // Step 3: 转回 JSON
    //         let edited = serde_json::to_value(hover).ok()?;
    //         Some(json!({ "result": edited }))
    //     }
    // }).await;

    dispatcher
        .register_from_backend("initialize", handle_initialize)
        .await;
}