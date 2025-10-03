# CodeFuse LSP Proxy

CodeFuse 是一个基于 Rust 的 LSP (Language Server Protocol) 代理服务器，充当 VSCode 和 clangd 语言服务器之间的中间层。它允许你拦截、修改和增强 LSP 消息流，提供自定义的语言服务器功能

## 功能特性

- 透明代理 LSP 消息
- 支持注册自定义处理器来修改请求和响应
- 异步处理，支持高并发
- 基于 tokio 的异步运行时

## 使用

CodeFuse 作为 LSP 服务器运行，可以配置在 VSCode 中使用

## 项目结构

```txt
src/
├── main.rs          # 主入口点，设置异步任务和处理器
├── dispatcher.rs    # 消息分发器，负责注册和处理 LSP 消息
├── clangd_client.rs # clangd 客户端，负责启动和管理 clangd 进程
├── tasks.rs         # 异步任务函数，处理数据收发
├── handlers.rs      # LSP 消息处理器，定义具体的处理逻辑
└── lib.rs           # 库文件（如果需要）
```

## 如何编写代码

### 注册 Dispatcher

CodeFuse 的核心功能是通过注册处理器来实现的。处理器允许你拦截和修改 LSP 消息

#### 基本概念

- **Dispatcher**: 消息分发器，管理所有注册的处理器
- **Handler**: 处理函数，用于处理特定的 LSP 方法
- **Sender**: 用于发送消息到前端（VSCode）或后端（clangd）

#### 定义处理器函数

首先定义处理器函数：

```rust
use futures::future::BoxFuture;
use tokio::sync::mpsc;
use serde_json::Value;
use anyhow::Result;

fn handle_initialize(
    rpc: Value,
    frontend_sender: mpsc::UnboundedSender<String>,
) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move {
        // 处理逻辑
        // rpc 是接收到的 JSON 消息
        // frontend_sender 用于发送消息到 VSCode

        // 示例：修改初始化响应
        let mut raw_rpc = rpc.clone();
        let raw_result = rpc
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Missing result field"))?;
        let mut init_result: InitializeResult = serde_json::from_value(raw_result)?;

        // 修改服务器信息
        init_result.server_info = Some(ServerInfo {
            name: "codefuse".into(),
            version: Some("0.1.0".into()),
        });

        let edited = serde_json::to_value(init_result)?;

        if let Some(obj) = raw_rpc.as_object_mut() {
            obj.insert("result".to_string(), edited);
        }

        let message = Dispatcher::format_lsp_message(&raw_rpc)?;
        frontend_sender.send(message)?;
        Ok(())
    })
}
```

#### 注册处理器

在 `setup_handlers` 函数中注册处理器：

```rust
pub async fn setup_handlers(dispatcher: Arc<Dispatcher>) {
    // 注册从后端（clangd）到前端（VSCode）的处理器
    dispatcher
        .register_from_backend("initialize", handle_initialize)
        .await;

    // 注册从前端（VSCode）到后端（clangd）的处理器
    dispatcher
        .register_from_frontend("textDocument/hover", handle_hover)
        .await;
}
```

#### 处理器签名

处理器函数的签名如下：

```rust
fn(Value, UnboundedSender<String>) -> BoxFuture<'static, Result<()>>
```

- `Value`: 接收到的 JSON-RPC 消息
- `UnboundedSender<String>`: 用于发送格式化的 LSP 消息
- 返回: `BoxFuture<'static, Result<()>>` 的 Future

#### 消息格式

LSP 消息使用 JSON-RPC 2.0 格式，包含：

- 请求: `{"jsonrpc": "2.0", "id": 1, "method": "method_name", "params": {...}}`
- 响应: `{"jsonrpc": "2.0", "id": 1, "result": {...}}`
- 通知: `{"jsonrpc": "2.0", "method": "method_name", "params": {...}}`

#### 实用工具

`Dispatcher` 提供了几个实用方法：

- `format_lsp_message(&Value) -> Result<String>`: 将 JSON 格式化为 LSP 消息（包含 Content-Length 头）
- `format_notification_or_request(&Value) -> Value`: 格式化通知或请求
- `format_result(Value) -> Value`: 格式化结果响应

### 添加新功能

1. 在 `handlers.rs` 中定义新的处理器函数
2. 在 `setup_handlers` 中注册新的处理器
3. 使用 `serde_json` 解析和构造 JSON 消息
4. 使用 `tower_lsp` 类型来处理 LSP 特定的数据结构
5. 处理错误并返回适当的响应

处理器函数应该遵循以下模式：

```rust
fn handle_your_method(
    rpc: Value,
    sender: mpsc::UnboundedSender<String>,
) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move {
        // 你的处理逻辑
        // ...
        Ok(())
    })
}
```

### 示例：增强悬停信息

```rust
fn handle_hover(
    rpc: Value,
    frontend_sender: mpsc::UnboundedSender<String>,
) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move {
        let mut raw_rpc = rpc.clone();
        let raw_result = rpc.get("result").cloned().unwrap_or(json!(null));

        // 解析为 Hover 类型
        if let Ok(mut hover) = serde_json::from_value::<Hover>(raw_result) {
            // 增强悬停内容
            match &mut hover.contents {
                HoverContents::Scalar(MarkedString::String(s)) => {
                    s.push_str("\n\n---\nEnhanced by CodeFuse");
                }
                HoverContents::Array(arr) => {
                    arr.push(MarkedString::String("Enhanced by CodeFuse".into()));
                }
                _ => {}
            }

            let edited = serde_json::to_value(hover)?;
            if let Some(obj) = raw_rpc.as_object_mut() {
                obj.insert("result".to_string(), edited);
            }
        }

        let message = Dispatcher::format_lsp_message(&raw_rpc)?;
        frontend_sender.send(message)?;
        Ok(())
    })
}

// 在 setup_handlers 中注册
dispatcher
    .register_from_backend("textDocument/hover", handle_hover)
    .await;
```
