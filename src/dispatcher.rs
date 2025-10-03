use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;

type DispatcherFn =
    Box<dyn Fn(Value, Sender<String>) -> BoxFuture<'static, Result<()>> + Send + Sync>;

pub struct Dispatcher {
    backend_handlers: Mutex<HashMap<String, DispatcherFn>>,
    frontend_handlers: Mutex<HashMap<String, DispatcherFn>>,
    backend_sender: Sender<String>,
    frontend_sender: Sender<String>,
    pending_requests: Mutex<HashMap<u64, String>>,
}

impl Dispatcher {
    pub fn new(backend_sender: Sender<String>, frontend_sender: Sender<String>) -> Self {
        Self {
            backend_handlers: Mutex::new(HashMap::new()),
            frontend_handlers: Mutex::new(HashMap::new()),
            backend_sender,
            frontend_sender,
            pending_requests: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register_from_frontend<F, Fut>(&self, method: &str, handler: F)
    where
        F: Fn(Value, Sender<String>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let boxed: DispatcherFn =
            Box::new(move |rpc, backend_sender| Box::pin(handler(rpc, backend_sender.clone())));
        self.backend_handlers
            .lock()
            .await
            .insert(method.to_string(), boxed);
    }

    pub async fn register_from_backend<F, Fut>(&self, method: &str, handler: F)
    where
        F: Fn(Value, Sender<String>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let boxed: DispatcherFn =
            Box::new(move |rpc, frontend_sender| Box::pin(handler(rpc, frontend_sender.clone())));
        self.frontend_handlers
            .lock()
            .await
            .insert(method.to_string(), boxed);
    }

    pub async fn handle_from_frontend(&self, rpc: Value) -> Result<()> {
        // 如果是请求（有 id 和 method），记录到字典
        if let (Some(id_val), Some(method_val)) = (rpc.get("id"), rpc.get("method")) {
            if let (Some(id), Some(method)) = (id_val.as_u64(), method_val.as_str()) {
                self.pending_requests
                    .lock()
                    .await
                    .insert(id, method.to_string());
            }
        }

        let method = rpc.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if let Some(handler) = self.backend_handlers.lock().await.get(method) {
            handler(rpc, self.backend_sender.clone()).await
        } else {
            let message = Self::format_lsp_message(&rpc)?;
            self.backend_sender.send(message).await?;
            Ok(())
        }
    }

    pub async fn handle_from_backend(&self, rpc: Value) -> Result<()> {
        // 统一获取 method：如果是响应，从字典中查找；如果是通知，从消息中获取
        let method = if let Some(id_val) = rpc.get("id") {
            if let Some(id) = id_val.as_u64() {
                let mut pending = self.pending_requests.lock().await;
                pending.remove(&id) // 获取并移除
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
            if let Some(handler) = self.frontend_handlers.lock().await.get(&method) {
                return handler(rpc, self.frontend_sender.clone()).await;
            }
        }

        let message = Self::format_lsp_message(&rpc)?;
        self.frontend_sender.send(message).await?;
        Ok(())
    }

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

    pub fn format_lsp_message(result: &Value) -> Result<String> {
        let body = serde_json::to_string(&result)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let message = format!("{}{}", header, body);
        Ok(message)
    }
}
