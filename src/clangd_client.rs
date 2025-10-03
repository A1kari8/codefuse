//! # Clangd 客户端模块
//!
//! 这个模块提供了与 clangd 语言服务器进程交互的功能。
//! 它负责启动 clangd 进程，并提供标准输入输出的句柄用于通信。

use std::sync::atomic::AtomicU64;
use tokio::io::BufReader;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use log::{debug, error, info, warn};
use tokio::io::AsyncBufReadExt;

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

pub async fn pipe_clangd_stderr(stderr: BufReader<ChildStderr>) {
    let mut lines = stderr.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        // 示例：I[11:01:38.638] clangd version 21.1.0
        let trimmed = line.trim();

        if let Some((level, rest)) = parse_clangd_log_line(trimmed) {
            match level {
                'I' => info!("[clangd] {}", rest),
                'W' => warn!("[clangd] {}", rest),
                'E' => error!("[clangd] {}", rest),
                'F' => error!("[clangd] FATAL: {}", rest),
                _ => debug!("[clangd] {}", trimmed),
            }
        } else {
            debug!("[clangd] {}", trimmed); // 无法解析，降级为 debug
        }
    }
}

fn parse_clangd_log_line(line: &str) -> Option<(char, &str)> {
    if line.len() >= 15 {
        let level = line.chars().next()?; // 第一个字符是日志等级
        let rest = &line[15..]; // 跳过前缀
        Some((level, rest.trim()))
    } else {
        None
    }
}
