//! # 调度器模块
//!
//! 这个模块实现了消息调度器，用于在前端（VSCode）和后端（clangd）之间分发和处理 LSP 消息。
//! 它支持注册自定义处理器来拦截和修改特定类型的消息。

use anyhow::Result;
use dashmap::DashMap;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tokio::sync::mpsc::UnboundedSender;
use tower_lsp::lsp_types::request;
use tower_lsp::lsp_types::notification;

/// 调度器函数类型别名。
///
/// 这个类型表示一个异步处理器函数，它接收一个 JSON 值和一个发送器，
/// 返回一个表示操作结果的 `BoxFuture`。
type DispatcherFn = fn(Value, UnboundedSender<String>) -> BoxFuture<'static, Result<()>>;

/// 消息调度器结构体。
///
/// 调度器负责管理前端和后端之间的消息流，包括：
/// - 注册来自前端和后端的处理器
/// - 处理传入的消息
/// - 转发未处理的消息
/// - 管理待处理的请求
pub struct Dispatcher {
    handlers_from_frontend: RwLock<HashMap<String, DispatcherFn>>,
    handlers_from_backend: RwLock<HashMap<String, DispatcherFn>>,
    backend_sender: UnboundedSender<String>,
    frontend_sender: UnboundedSender<String>,
    pending_requests: DashMap<u64, String>,
}

impl Dispatcher {
    /// 创建新的调度器实例。
    ///
    /// # 参数
    ///
    /// * `backend_sender` - 向后端发送消息的通道发送器
    /// * `frontend_sender` - 向前端发送消息的通道发送器
    ///
    /// # 返回
    ///
    /// 返回初始化后的 `Dispatcher` 实例
    pub fn new(
        backend_sender: UnboundedSender<String>,
        frontend_sender: UnboundedSender<String>,
    ) -> Self {
        Self {
            handlers_from_frontend: RwLock::new(HashMap::new()),
            handlers_from_backend: RwLock::new(HashMap::new()),
            backend_sender,
            frontend_sender,
            pending_requests: DashMap::new(),
        }
    }

    /// 注册来自前端的处理器。
    ///
    /// 这个方法允许为特定的 LSP 方法注册异步处理器函数。
    /// 当从前端接收到匹配该方法的消息时，将调用注册的处理器。
    ///
    /// # 参数
    ///
    /// * `handler` - 处理函数，接收消息和后端发送器
    ///
    /// # 类型参数
    ///
    /// * `T` - LSP 请求类型，必须实现 Request trait
    pub async fn register_req_from_frontend<T>(&self, handler: DispatcherFn)
    where
        T: request::Request,
    {
        let method = T::METHOD;
        self.handlers_from_frontend
            .write()
            .await
            .insert(method.to_string(), handler);
    }

    pub async fn register_notification_from_frontend<T>(&self, handler: DispatcherFn)
    where
        T: notification::Notification,
    {
        let method = T::METHOD;
        self.handlers_from_frontend
            .write()
            .await
            .insert(method.to_string(), handler);
    }

    /// 注册来自后端的处理器。
    ///
    /// 这个方法允许为特定的 LSP 方法注册异步处理器函数。
    /// 当从后端接收到匹配该方法的消息时，将调用注册的处理器。
    ///
    /// # 参数
    ///
    /// * `handler` - 处理函数，接收消息和前端发送器
    ///
    /// # 类型参数
    ///
    /// * `T` - LSP 请求类型，必须实现 Request trait
    pub async fn register_req_resp_from_backend<T>(&self, handler: DispatcherFn)
    where
        T: request::Request,
    {
        let method = T::METHOD;
        self.handlers_from_backend
            .write()
            .await
            .insert(method.to_string(), handler);
    }

    pub async fn register_notification_from_backend<T>(&self, handler: DispatcherFn)
    where
        T: notification::Notification,
    {
        let method = T::METHOD;
        self.handlers_from_backend
            .write()
            .await
            .insert(method.to_string(), handler);
    }

    /// 处理来自前端的消息。
    ///
    /// 这个方法接收来自前端的 JSON-RPC 消息，检查是否有注册的处理器，
    /// 如果有则调用处理器，否则将消息转发给后端。
    /// 对于请求消息，还会记录到待处理请求字典中。
    ///
    /// # 参数
    ///
    /// * `rpc` - 接收到的 JSON-RPC 消息
    ///
    /// # 返回
    ///
    /// 返回 `Result<()>`，表示处理是否成功
    pub async fn handle_from_frontend(&self, rpc: Value) -> Result<()> {
        // 如果是请求（有 id 和 method），记录到字典
        if let (Some(id_val), Some(method_val)) = (rpc.get("id"), rpc.get("method")) {
            if let (Some(id), Some(method)) = (id_val.as_u64(), method_val.as_str()) {
                self.pending_requests.insert(id, method.to_string());
            }
        }

        let method = rpc.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if let Some(handler) = self.handlers_from_frontend.read().await.get(method) {
            handler(rpc, self.backend_sender.clone()).await
        } else {
            let message = Self::format_lsp_message(&rpc)?;
            self.backend_sender.send(message)?;
            Ok(())
        }
    }

    /// 处理来自后端的消息。
    ///
    /// 这个方法接收来自后端的 JSON-RPC 消息，确定消息类型（响应或通知），
    /// 检查是否有注册的处理器，如果有则调用处理器，否则将消息转发给前端。
    ///
    /// # 参数
    ///
    /// * `rpc` - 接收到的 JSON-RPC 消息
    ///
    /// # 返回
    ///
    /// 返回 `Result<()>`，表示处理是否成功
    pub async fn handle_from_backend(&self, rpc: Value) -> Result<()> {
        // 统一获取 method：如果是响应，从字典中查找；如果是通知，从消息中获取
        let method = if let Some(id_val) = rpc.get("id") {
            if let Some(id) = id_val.as_u64() {
                self.pending_requests.remove(&id).map(|(_, v)| v) // 获取并移除
            } else {
                None
            }
        } else {
            rpc.get("method")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        };

        // 如果有 method 且注册了处理器，调用；否则直接转发
        if let Some(method) = method {
            if let Some(handler) = self.handlers_from_backend.read().await.get(&method) {
                return handler(rpc, self.frontend_sender.clone()).await;
            }
        }

        let message = Self::format_lsp_message(&rpc)?;
        self.frontend_sender.send(message)?;
        Ok(())
    }

    /// 格式化通知或请求消息。
    ///
    /// 根据消息是否包含 `id` 字段，将其格式化为标准的 JSON-RPC 通知或请求。
    ///
    /// # 参数
    ///
    /// * `rpc` - 要格式化的原始消息
    ///
    /// # 返回
    ///
    /// 返回格式化后的 JSON 值
    pub fn format_notification_or_request(rpc: &Value) -> Value {
        let params = rpc.get("params").cloned().unwrap_or(json!(null));
        let method = rpc.get("method").cloned().unwrap_or(json!(null));
        match rpc.get("id") {
            Some(id) => {
                let request = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": method,
                    "params": params
                });
                request
            }
            None => {
                let notification = json!({
                    "jsonrpc": "2.0",
                    "method": method,
                    "params": params
                });
                notification
            }
        }
    }
    /// 格式化结果消息。
    ///
    /// 从参数中提取方法、ID 和参数，构建标准的 JSON-RPC 结果响应。
    ///
    /// # 参数
    ///
    /// * `rpc` - 包含结果信息的原始消息
    ///
    /// # 返回
    ///
    /// 返回格式化后的 JSON 值
    pub fn format_result(rpc: Value) -> Value {
        let params = rpc.get("params").cloned().unwrap_or(json!(null));
        let method = params.get("method").cloned().unwrap_or(json!(null));
        let id = params.get("id").cloned().unwrap_or(json!(null));

        let result = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        result
    }

    /// 格式化 LSP 消息为字符串。
    ///
    /// 将 JSON 值序列化为字符串，并添加 LSP 协议要求的 Content-Length 头部。
    ///
    /// # 参数
    ///
    /// * `result` - 要格式化的 JSON 值
    ///
    /// # 返回
    ///
    /// 返回格式化后的 LSP 消息字符串
    ///
    /// # 错误
    ///
    /// 如果 JSON 序列化失败，返回错误
    pub fn format_lsp_message(result: &Value) -> Result<String> {
        let body = serde_json::to_string(&result)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let message = format!("{}{}", header, body);
        Ok(message)
    }
}
