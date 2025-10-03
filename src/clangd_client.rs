//! # Clangd 客户端模块
//!
//! 这个模块提供了与 clangd 语言服务器进程交互的功能。
//! 它负责启动 clangd 进程，并提供标准输入输出的句柄用于通信。

use std::sync::atomic::AtomicU64;
use tokio::io::BufReader;
use tokio::process::{ChildStdin, ChildStdout,ChildStderr,Command};

/// Clangd 客户端结构体。
///
/// 这个结构体封装了与 clangd 进程通信所需的所有组件：
/// - `stdin`: 用于向 clangd 发送数据的标准输入句柄
/// - `stdout`: 用于从 clangd 接收数据的标准输出缓冲读取器
/// - `id_counter`: 用于生成唯一的请求 ID 的原子计数器
pub struct ClangdClient {
    pub stdin: ChildStdin,
    pub stdout: BufReader<ChildStdout>,
    pub stderr: BufReader<ChildStderr>,
    pub id_counter: AtomicU64,
}

impl ClangdClient {
    /// 启动新的 clangd 进程并创建客户端实例。
    ///
    /// 这个方法执行以下操作：
    /// 1. 使用 `Command::new("clangd")` 创建新的进程
    /// 2. 设置标准输入和输出为管道
    /// 3. 启动进程并获取输入输出句柄
    /// 4. 初始化 ID 计数器为 1
    ///
    /// # 返回
    ///
    /// 返回初始化后的 `ClangdClient` 实例
    ///
    /// # 恐慌
    ///
    /// 如果无法启动 clangd 进程，将会恐慌
    pub async fn spawn() -> Self {
        let mut child = Command::new("clangd")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("Failed to start clangd");

        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let stderr = BufReader::new(child.stderr.take().unwrap());

        Self {
            stdin,
            stdout,
            stderr,
            id_counter: AtomicU64::new(1),
        }
    }
}
