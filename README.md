# CodeFuse LSP Proxy

CodeFuse 是一个基于 Rust 的 LSP (Language Server Protocol) 代理服务器，充当 VSCode 和 clangd 语言服务器之间的中间层。它允许你拦截、修改和增强 LSP 消息流，提供自定义的语言服务器功能

## 功能特性

- 透明代理 LSP 消息
- 支持注册自定义处理器来修改请求和响应
- 异步处理，支持高并发
- 基于 tokio 的异步运行时
- 编译时类型安全的处理器注册

## 依赖

CodeFuse 依赖以下主要 crate：

- `tokio`: 异步运行时
- `tower-lsp`: LSP 类型定义和协议处理
- `serde_json`: JSON 序列化和反序列化
- `futures`: 异步 Future 工具
- `anyhow`: 错误处理
- `dashmap`: 并发安全的 HashMap

## 使用

CodeFuse 作为 LSP 服务器运行，可以配置在 VSCode 中使用

## 项目结构

```txt
src/
├── main.rs          # 主入口点，设置异步任务和处理器
├── dispatcher.rs    # 消息分发器，负责注册和处理 LSP 消息（请求和通知）
├── clangd_client.rs # clangd 客户端，负责启动和管理 clangd 进程
├── tasks.rs         # 异步任务函数，处理数据收发
└── handlers.rs      # LSP 消息处理器，定义具体的处理逻辑
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
use tower_lsp::lsp_types::{InitializeResult, ServerInfo};

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

#### 处理器签名

处理器函数的签名如下：

```rust
fn(Value, UnboundedSender<String>) -> BoxFuture<'static, Result<()>>
```

- `Value`: 接收到的 JSON-RPC 消息
- `UnboundedSender<String>`: 用于发送格式化的 LSP 消息
- 返回: `BoxFuture<'static, Result<()>>` 的 Future

所有处理器都使用相同的签名，无论处理请求还是通知

#### 注册方法

注册方法使用 tower-lsp 的请求和通知类型作为类型参数：

```rust
// 请求处理器
dispatcher.register_req_resp_from_backend::<Initialize>(handler).await;      // 处理初始化响应
dispatcher.register_req_from_frontend::<HoverRequest>(handler).await;   // 处理悬停请求

// 通知处理器
dispatcher.register_notification_from_frontend::<DidOpenTextDocument>(handler).await;  // 处理文档打开通知
dispatcher.register_notification_from_backend::<PublishDiagnostics>(handler).await;    // 处理诊断通知
```

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

### 示例：处理文档打开通知

```rust
use tower_lsp::lsp_types::DidOpenTextDocumentParams;
use serde_json::json;

fn handle_did_open(
    rpc: Value,
    backend_sender: mpsc::UnboundedSender<String>,
) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move {
        // 解析文档打开通知
        if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(rpc.get("params").cloned().unwrap_or(json!(null))) {
            info!("Document opened: {}", params.text_document.uri);

            // 可以在这里进行一些处理，比如语法检查等
            // 然后转发给后端
            let message = Dispatcher::format_lsp_message(&rpc)?;
            backend_sender.send(message)?;
        }
        Ok(())
    })
}

// 在 setup_handlers 中注册
dispatcher
    .register_from_frontend_notification::<DidOpenTextDocument>(handle_did_open)
    .await;
```
