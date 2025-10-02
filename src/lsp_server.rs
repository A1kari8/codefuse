/// LspServer trait 定义了语言服务器协议（Language Server Protocol）的核心接口
/// 
/// 这个 trait 抽象了与不同语言服务器（如 clangd, rust-analyzer 等）交互的通用接口。
/// 通过实现这个 trait，可以统一处理不同语言服务器的通信，实现多语言支持。
/// 
/// 该 trait 需要实现 Send + Sync，以支持在多线程环境中安全使用。
/// 
/// # 主要功能
/// 
/// - 悬停信息查询：获取代码符号的详细信息
/// - 代码补全：提供智能代码补全建议
/// - 语义令牌：为语法高亮提供语义信息
/// - 通知发送：向语言服务器发送各种通知消息
#[async_trait::async_trait]
pub trait LspServer: Send + Sync {
    /// 发送悬停（hover）请求到语言服务器
    /// 
    /// 当用户将鼠标悬停在代码符号上时调用，用于获取该符号的详细信息，
    /// 如类型信息、文档注释、函数签名等。
    /// 
    /// # Arguments
    /// 
    /// * `file_uri` - 目标文件的 URI，通常格式为 "file:///path/to/file"
    /// * `line` - 光标所在行号（从0开始计数）
    /// * `character` - 光标在该行的字符位置（从0开始计数）
    /// 
    /// # Returns
    /// 
    /// 返回包含悬停信息的 JSON 格式字符串，遵循 LSP 协议规范
    async fn send_hover(&mut self, file_uri: &str, line: u32, character: u32) -> String;

    /// 发送代码补全（completion）请求到语言服务器
    /// 
    /// 当用户输入代码时触发，用于获取可能的代码补全建议，
    /// 包括变量名、函数名、类型名等智能提示。
    /// 
    /// # Arguments
    /// 
    /// * `file_uri` - 目标文件的 URI
    /// * `line` - 光标所在行号（从0开始计数）
    /// * `character` - 光标在该行的字符位置（从0开始计数）
    /// 
    /// # Returns
    /// 
    /// 返回包含补全建议列表的 JSON 格式字符串，遵循 LSP 协议规范
    async fn send_completion(&mut self, file_uri: &str, line: u32, character: u32) -> String;

    /// 发送语义令牌（semantic tokens）请求到语言服务器
    /// 
    /// 用于获取文件的语义令牌信息，这些信息可以用于实现更精确的语法高亮，
    /// 区分不同的语义元素（如变量、函数、类型、关键字等）。
    /// 
    /// # Arguments
    /// 
    /// * `file_uri` - 目标文件的 URI
    /// 
    /// # Returns
    /// 
    /// 返回包含语义令牌数据的 JSON 格式字符串，数据格式遵循 LSP 协议规范
    async fn send_semantic_tokens(&mut self, file_uri: &str) -> String;

    /// 向语言服务器发送通知消息
    /// 
    /// 通知是单向消息，不期望服务器返回响应。常用于通知服务器
    /// 文档的打开、关闭、修改等状态变化。
    /// 
    /// # Arguments
    /// 
    /// * `notification` - 要发送的通知消息，应为完整的 LSP 格式消息
    ///                   包含 Content-Length 头部和 JSON 消息体
    /// 
    /// # Returns
    /// 
    /// 返回 `Ok(())` 表示通知发送成功，`Err` 表示发送过程中出现 I/O 错误
    /// 
    /// # Errors
    /// 
    /// 当底层 I/O 操作失败时返回 `std::io::Error`
    async fn send_notification(&mut self, notification: &str) -> Result<(), std::io::Error>;

    /// 发送请求到语言服务器并等待响应
    /// 
    /// 请求是双向消息，期望服务器返回响应。用于需要服务器回复的操作，
    /// 如初始化、文档符号查询等。
    /// 
    /// # Arguments
    /// 
    /// * `request` - 要发送的请求消息，应为完整的 LSP 格式消息
    ///               包含 Content-Length 头部和 JSON 消息体
    /// 
    /// # Returns
    /// 
    /// 返回服务器的响应消息，作为 JSON 格式字符串
    /// 
    /// # Errors
    /// 
    /// 当底层 I/O 操作失败时返回 `std::io::Error`
    async fn send_request(&mut self, request: &str) -> Result<String, std::io::Error>;
}
