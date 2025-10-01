use crate::lsp_server::LspServer;

/// MockLspServer 是一个模拟的 LSP 服务器实现
/// 
/// 这个结构体实现了 LspServer trait，用于在测试或开发阶段
/// 提供模拟的语言服务器功能，而不需要启动真实的语言服务器进程。
/// 它返回预定义的响应，便于测试和调试。
pub struct MockLspServer;

impl MockLspServer {
    /// 创建一个新的 MockLspServer 实例
    /// 
    /// # Returns
    /// 
    /// 返回一个新的 MockLspServer 实例
    pub fn new() -> Self {
        MockLspServer
    }
}

#[async_trait::async_trait]
impl LspServer for MockLspServer {
    /// 发送悬停请求并返回模拟的悬停信息
    /// 
    /// 这个方法模拟真实 LSP 服务器的悬停功能，返回固定的 JSON 响应。
    /// 在实际的 LSP 服务器中，这会返回光标位置处符号的详细信息。
    /// 
    /// # Arguments
    /// 
    /// * `_file_uri` - 文件的 URI（在模拟实现中未使用）
    /// * `_line` - 光标所在行号（在模拟实现中未使用）
    /// * `_character` - 光标所在字符位置（在模拟实现中未使用）
    /// 
    /// # Returns
    /// 
    /// 返回包含模拟悬停信息的 JSON 字符串
    async fn send_hover(&mut self, _file_uri: &str, _line: u32, _character: u32) -> String {
        r#"{"jsonrpc": "2.0", "id": 1, "result": {"contents": {"kind": "markdown", "value": "Mock hover info"}}}"#.to_string()
    }

    /// 发送代码补全请求并返回模拟的补全建议
    /// 
    /// 这个方法模拟真实 LSP 服务器的代码补全功能，返回预定义的补全项列表。
    /// 在实际的 LSP 服务器中，这会根据上下文返回相关的补全建议。
    /// 
    /// # Arguments
    /// 
    /// * `_file_uri` - 文件的 URI（在模拟实现中未使用）
    /// * `_line` - 光标所在行号（在模拟实现中未使用）
    /// * `_character` - 光标所在字符位置（在模拟实现中未使用）
    /// 
    /// # Returns
    /// 
    /// 返回包含模拟补全建议的 JSON 字符串
    /// kind: 3 表示函数类型的补全项
    async fn send_completion(&mut self, _file_uri: &str, _line: u32, _character: u32) -> String {
        r#"{"jsonrpc": "2.0", "id": 2, "result": {"items": [{"label": "mock_function", "kind": 3, "detail": "Mock function"}]}}"#.to_string()
    }

    /// 发送语义令牌请求并返回模拟的语义令牌数据
    /// 
    /// 这个方法模拟真实 LSP 服务器的语义令牌功能，用于语法高亮。
    /// 返回的数据格式为 [deltaLine, deltaStartChar, length, tokenType, tokenModifiers]
    /// 
    /// # Arguments
    /// 
    /// * `_file_uri` - 文件的 URI（在模拟实现中未使用）
    /// 
    /// # Returns
    /// 
    /// 返回包含模拟语义令牌数据的 JSON 字符串
    /// data: [0, 0, 4, 0, 0] 表示从第0行第0列开始，长度为4的令牌
    async fn send_semantic_tokens(&mut self, _file_uri: &str) -> String {
        r#"{"jsonrpc": "2.0", "id": 3, "result": {"data": [0, 0, 4, 0, 0]}}"#.to_string()
    }

    /// 发送通知消息
    /// 
    /// 这个方法模拟向 LSP 服务器发送通知的功能。
    /// 通知是单向的，不期望返回响应。
    /// 在模拟实现中，这个方法什么都不做，直接返回成功。
    /// 
    /// # Arguments
    /// 
    /// * `_notification` - 要发送的通知消息（在模拟实现中未使用）
    /// 
    /// # Returns
    /// 
    /// 返回 Ok(()) 表示通知发送成功
    async fn send_notification(&mut self, _notification: &str) -> Result<(), std::io::Error> {
        Ok(())
    }
}
