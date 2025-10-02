//! clangd.rs - Clangd 语言服务器会话管理模块
//!
//! 这个模块实现了与 clangd 语言服务器的通信接口。clangd 是 LLVM 项目提供的
//! C/C++ 语言服务器，支持代码补全、悬停信息、语义令牌等 LSP 功能。
//!
//! # 主要组件
//!
//! - `ClangdSession`: 管理与 clangd 进程的通信会话
//! - LSP 协议实现：处理 JSON-RPC 消息的发送和接收
//! - 异步 I/O：使用 tokio 实现非阻塞的进程通信

use crate::lsp_server::LspServer;

// 标准库导入：原子操作和内存排序
use std::sync::atomic::{AtomicU32, Ordering};
// tokio 异步 I/O 导入：缓冲读取、异步读写操作
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
// tokio 进程管理：子进程标准输入和命令执行
use tokio::process::{ChildStdin, Command};
// tokio 时间操作：超时控制
use tokio::time::{Duration, timeout};

/// ClangdSession 管理与 clangd 语言服务器进程的通信会话
///
/// 这个结构体封装了与 clangd 进程的双向通信管道，负责：
/// - 启动和管理 clangd 子进程
/// - 发送 LSP 请求消息到 clangd
/// - 接收和解析 clangd 的响应
/// - 维护请求 ID 的唯一性
///
/// # 通信协议
///
/// 使用 Language Server Protocol (LSP) 进行通信，消息格式为：
/// ```text
/// Content-Length: <字节数>\r\n
/// \r\n
/// <JSON 消息体>
/// ```
///
/// # 线程安全
///
/// 该结构体通过原子操作确保请求 ID 的线程安全递增。
pub struct ClangdSession {
    /// clangd 进程的标准输入管道，用于发送 LSP 消息
    stdin: ChildStdin,
    /// clangd 进程的标准输出缓冲读取器，用于接收响应
    reader: BufReader<tokio::process::ChildStdout>,
    /// 原子递增的请求 ID，确保每个请求的唯一标识
    id: AtomicU32,
}

impl ClangdSession {
    pub(crate) async fn new() -> Result<Self, std::io::Error> {
        let mut child = Command::new("clangd")
            .arg("--log=verbose")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?; // 用 ? 传播错误

        let stdin = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to open stdin")
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to open stdout")
        })?;

        let reader = BufReader::new(stdout);

        Ok(ClangdSession {
            stdin,
            reader,
            id: AtomicU32::new(5),
        })
    }

    /// 发送悬停信息请求到 clangd
    ///
    /// 这个方法会构造一个 LSP textDocument/hover 请求，用于获取
    /// 指定位置的代码符号的详细信息。
    ///
    /// # Arguments
    ///
    /// * `file_uri` - 文件的 URI，格式如 "file:///path/to/file.cpp"
    /// * `line` - 光标所在行号（从 0 开始计数）
    /// * `character` - 光标在该行的字符位置（从 0 开始计数）
    ///
    /// # Returns
    ///
    /// 返回 clangd 的响应 JSON 字符串，包含：
    /// - 成功：符号的类型信息、文档注释等
    /// - 失败：以 "error:" 开头的错误信息
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let response = session.send_hover("file:///test.cpp", 10, 5).await;
    /// // 可能返回：{"jsonrpc":"2.0","id":6,"result":{"contents":{"kind":"markdown","value":"int x"}}}
    /// ```
    /// 发送悬停信息请求到 clangd
    ///
    /// 这个方法会构造一个 LSP textDocument/hover 请求，用于获取
    /// 指定位置的代码符号的详细信息。
    ///
    /// # Arguments
    ///
    /// * `file_uri` - 文件的 URI，格式如 "file:///path/to/file.cpp"
    /// * `line` - 光标所在行号（从 0 开始计数）
    /// * `character` - 光标在该行的字符位置（从 0 开始计数）
    ///
    /// # Returns
    ///
    /// 返回 clangd 的响应 JSON 字符串，包含：
    /// - 成功：符号的类型信息、文档注释等
    /// - 失败：以 "error:" 开头的错误信息
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let response = session.send_hover("file:///test.cpp", 10, 5).await;
    /// // 可能返回：{"jsonrpc":"2.0","id":6,"result":{"contents":{"kind":"markdown","value":"int x"}}}
    /// ```
    pub(crate) async fn send_hover(&mut self, file_uri: &str, line: u32, character: u32) -> String {
        let id = self.id.fetch_add(1, Ordering::SeqCst);
        let payload = format!(
            r#"{{
            "jsonrpc": "2.0",
            "id": {},
            "method": "textDocument/hover",
            "params": {{
                "textDocument": {{ "uri": "{}" }},
                "position": {{ "line": {}, "character": {} }}
            }}
        }}"#,
            id, file_uri, line, character
        );

        let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        match self.send_request(&request, id).await {
            Ok(response) => response,
            Err(e) => format!("error: {}", e),
        }
    }

    /// 发送代码补全请求到 clangd
    ///
    /// 这个方法会构造一个 LSP textDocument/completion 请求，用于获取
    /// 指定位置的代码补全建议。
    ///
    /// # Arguments
    ///
    /// * `file_uri` - 文件的 URI，格式如 "file:///path/to/file.cpp"
    /// * `line` - 光标所在行号（从 0 开始计数）
    /// * `character` - 光标在该行的字符位置（从 0 开始计数）
    ///
    /// # Returns
    ///
    /// 返回 clangd 的响应 JSON 字符串，包含：
    /// - 成功：补全项列表，包括标签、类型、描述等
    /// - 失败：以 "error:" 开头的错误信息
    ///
    /// # clangd 补全特性
    ///
    /// clangd 提供的补全功能包括：
    /// - 函数名补全
    /// - 变量名补全
    /// - 类型名补全
    /// - 成员变量和函数补全
    /// - 宏定义补全
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let response = session.send_completion("file:///test.cpp", 15, 8).await;
    /// // 可能返回多个补全项的 JSON 数组
    /// ```
    pub(crate) async fn send_completion(
        &mut self,
        file_uri: &str,
        line: u32,
        character: u32,
    ) -> String {
        let id = self.id.fetch_add(1, Ordering::SeqCst);
        let payload = format!(
            r#"{{
            "jsonrpc": "2.0",
            "id": {},
            "method": "textDocument/completion",
            "params": {{
                "textDocument": {{ "uri": "{}" }},
                "position": {{ "line": {}, "character": {} }}
            }}
        }}"#,
            id, file_uri, line, character
        );

        let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        match self.send_request(&request, id).await {
            Ok(response) => response,
            Err(e) => format!("error: {}", e),
        }
    }

    /// 发送语义令牌请求到 clangd
    ///
    /// 这个方法会构造一个 LSP textDocument/semanticTokens/full 请求，
    /// 用于获取整个文件的语义令牌信息。
    ///
    /// # Arguments
    ///
    /// * `file_uri` - 文件的 URI，格式如 "file:///path/to/file.cpp"
    ///
    /// # Returns
    ///
    /// 返回 clangd 的响应 JSON 字符串，包含：
    /// - 成功：语义令牌数据数组，用于语法高亮
    /// - 失败：以 "error:" 开头的错误信息
    ///
    /// # 语义令牌格式
    ///
    /// 返回的数据为五元组数组：[deltaLine, deltaStart, length, tokenType, tokenModifiers]
    /// - deltaLine：相对于上一个令牌的行偏移
    /// - deltaStart：在同一行中相对于上一个令牌的列偏移
    /// - length：令牌的字符长度
    /// - tokenType：令牌类型索引（如关键字、函数名等）
    /// - tokenModifiers：令牌修饰符位模式
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let response = session.send_semantic_tokens("file:///test.cpp").await;
    /// // 返回语义令牌数据，用于实现精确的语法高亮
    /// ```
    pub(crate) async fn send_semantic_tokens(&mut self, file_uri: &str) -> String {
        let id = self.id.fetch_add(1, Ordering::SeqCst);
        let payload = format!(
            r#"{{
            "jsonrpc": "2.0",
            "id": {},
            "method": "textDocument/semanticTokens/full",
            "params": {{
                "textDocument": {{ "uri": "{}" }}
            }}
        }}"#,
            id, file_uri
        );

        let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        match self.send_request(&request, id).await {
            Ok(response) => response,
            Err(e) => format!("error: {}", e),
        }
    }



    /// 发送 LSP 请求到 clangd 并等待匹配的响应
    ///
    /// 这个方法处理完整的请求-响应周期：
    /// 1. 发送格式化的 LSP 请求到 clangd
    /// 2. 循环读取响应直到找到匹配请求 ID 的响应
    /// 3. 跳过通知和其他不匹配的响应
    /// 4. 实现 5 秒超时机制防止无限等待
    ///
    /// # Arguments
    ///
    /// * `request` - 完整的 LSP 请求字符串，包含 Content-Length 头部和 JSON 消息体
    /// * `expected_id` - 期望的响应 ID，用于匹配正确的响应
    ///
    /// # Returns
    ///
    /// 成功时返回 clangd 的 JSON 响应字符串，失败时返回 I/O 错误
    ///
    /// # Errors
    ///
    /// 在以下情况下会返回错误：
    /// - 发送请求到 clangd 失败
    /// - 读取响应超时（5秒）
    /// - clangd 返回无效的响应格式
    /// - JSON 解析失败
    /// - clangd 进程意外终止
    ///
    /// # 超时机制
    ///
    /// 每个读取操作都有 5 秒超时限制，避免因 clangd 无响应而无限等待
    ///
    /// # 响应匹配
    ///
    /// 只返回 ID 匹配 `expected_id` 的响应，其他响应（如通知）会被跳过
    pub(crate) async fn send_request(
        &mut self,
        request: &str,
        expected_id: u32,
    ) -> Result<String, std::io::Error> {
        self.stdin.write_all(request.as_bytes()).await?;
        self.stdin.flush().await?;
        loop {
            let mut header_line = String::new();
            match timeout(
                Duration::from_secs(5),
                (&mut self.reader).read_line(&mut header_line),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "read timeout",
                    ));
                }
            }
            if !header_line.starts_with("Content-Length: ") {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Invalid response header",
                ));
            }
            let length_str = header_line.trim_start_matches("Content-Length: ").trim();
            let length: usize = length_str.parse().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::Other, "Parse Content-Length")
            })?;
            let mut empty_line = String::new();
            match timeout(
                Duration::from_secs(5),
                (&mut self.reader).read_line(&mut empty_line),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "read timeout",
                    ));
                }
            }
            let mut buffer = vec![0; length];
            match timeout(Duration::from_secs(5), self.reader.read_exact(&mut buffer)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "read timeout",
                    ));
                }
            }
            let response = String::from_utf8_lossy(&buffer).to_string();
            // Parse JSON to check id
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&response) {
                if let Some(id) = value.get("id") {
                    if id.as_u64() == Some(expected_id as u64) {
                        return Ok(response);
                    }
                }
                // If no id or id doesn't match, continue loop (skip notifications or other responses)
            } else {
                // If not valid JSON, perhaps log and continue, but for now return error
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Invalid JSON response",
                ));
            }
        }
    }

    /// 向 clangd 发送通知消息
    ///
    /// 通知是单向消息，不期望 clangd 返回响应。常用于：
    /// - 通知文档打开 (textDocument/didOpen)
    /// - 通知文档修改 (textDocument/didChange)
    /// - 通知文档关闭 (textDocument/didClose)
    /// - 初始化完成通知 (initialized)
    ///
    /// # Arguments
    ///
    /// * `notification` - 完整的 LSP 通知消息，包含 Content-Length 头部和 JSON 消息体
    ///
    /// # Returns
    ///
    /// 成功时返回 `Ok(())`，失败时返回 I/O 错误
    ///
    /// # Errors
    ///
    /// 在以下情况下会返回错误：
    /// - 写入 clangd 标准输入失败
    /// - 刷新缓冲区失败
    /// - clangd 进程已终止
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let notification = format!(
    ///     "Content-Length: {}\r\n\r\n{}",
    ///     payload.len(),
    ///     r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"
    /// );
    /// session.send_notification(&notification).await?;
    /// ```
    pub(crate) async fn send_notification(
        &mut self,
        notification: &str,
    ) -> Result<(), std::io::Error> {
        self.stdin.write_all(notification.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

/// 为 ClangdSession 实现 LspServer trait
///
/// 这个实现将 ClangdSession 的内部方法封装为统一的 LspServer 接口，
/// 使得上层代码可以通过统一的接口与不同的语言服务器交互。
///
/// trait 实现方法直接委托给相应的内部实现方法，避免代码重复。
#[async_trait::async_trait]
impl LspServer for ClangdSession {
    async fn send_hover(&mut self, file_uri: &str, line: u32, character: u32) -> String {
        ClangdSession::send_hover(self, file_uri, line, character).await
    }

    async fn send_completion(&mut self, file_uri: &str, line: u32, character: u32) -> String {
        ClangdSession::send_completion(self, file_uri, line, character).await
    }

    async fn send_semantic_tokens(&mut self, file_uri: &str) -> String {
        ClangdSession::send_semantic_tokens(self, file_uri).await
    }

    async fn send_notification(&mut self, notification: &str) -> Result<(), std::io::Error> {
        ClangdSession::send_notification(self, notification).await
    }

    async fn send_request(&mut self, request: &str) -> Result<String, std::io::Error> {
        let id = self.id.fetch_add(1, Ordering::SeqCst);
        ClangdSession::send_request(self, request, id).await
    }
}
