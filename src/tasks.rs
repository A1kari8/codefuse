use anyhow::{Context, Result};
use log::{error, info};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Stdin, Stdout};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Semaphore, mpsc};
use serde_json::Value;

use crate::dispatcher::Dispatcher;

/// 向后端（clangd）发送数据的异步任务。
///
/// 这个函数从接收器接收消息，并将其发送到 clangd 进程的标准输入。
/// 它持续监听接收器，直到通道关闭。
///
/// # 参数
///
/// * `stdin` - clangd 进程的标准输入句柄
/// * `rx` - 从调度器接收消息的通道接收器
///
/// # 返回
///
/// 返回 `Result<()>`，表示操作是否成功
///
/// # 错误
///
/// 如果写入或刷新失败，将返回错误
pub async fn send_data_backend(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<String>,
) -> Result<()> {
    while let Some(message) = rx.recv().await {
        // 发送数据到外部程序
        stdin.write_all(message.as_bytes()).await?;
        stdin.flush().await?;
        info!("已发送: {}", message);
    }
    Ok(())
}

/// 从后端（clangd）接收数据的异步任务。
///
/// 这个函数读取 clangd 进程的标准输出，按照 LSP 协议解析消息头和消息体，
/// 然后将解析后的 JSON 消息传递给调度器进行处理。
///
/// # 参数
///
/// * `stdout` - clangd 进程的标准输出缓冲读取器
/// * `dispatcher` - 调度器实例，用于处理接收到的消息
///
/// # 返回
///
/// 返回 `Result<()>`，表示操作是否成功
///
/// # 错误
///
/// 如果读取、解析或处理消息失败，将返回错误
pub async fn receive_data_backend(
    stdout: BufReader<ChildStdout>,
    dispatcher: Arc<Dispatcher>,
    semaphore: Arc<Semaphore>,
) -> Result<()> {
    let mut reader = stdout;

    loop {
        // 1. 读取 header
        let mut content_length = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            let line = line.trim();

            if line.is_empty() {
                break; // header 结束
            }

            if let Some(cl) = line.strip_prefix("Content-Length:") {
                content_length = Some(
                    cl.trim()
                        .parse::<usize>()
                        .context("Content-Length 解析失败")?,
                );
            }
        }

        let content_length = match content_length {
            Some(len) => len,
            None => continue, // 没有 Content-Length，跳过
        };

        // 2. 读取 body
        let mut body_buf = vec![0u8; content_length];
        reader.read_exact(&mut body_buf).await?;
        // let body_str = String::from_utf8(body_buf).context("UTF-8 解码失败")?;

        // 3. 解析 JSON
        // let json_body: Value = serde_json::from_str(&body_str).context("JSON 解析失败")?;
        let json_body: Value = serde_json::from_slice(&body_buf).context("JSON 解析失败")?;

        // 限制并发：获取许可
        let permit = semaphore.clone().acquire_owned().await?;
        let _ = permit;

        // 5. 并发处理
        let dispatcher = dispatcher.clone();
        tokio::spawn(async move {
            if let Err(e) = dispatcher.handle_from_backend(json_body).await {
                error!("处理失败: {:?}", e);
            }
        });
    }
}

/// 向前端（VSCode）发送数据的异步任务。
///
/// 这个函数从接收器接收消息，并将其发送到标准输出，供 VSCode 读取。
/// 它持续监听接收器，直到通道关闭。
///
/// # 参数
///
/// * `stdout` - 标准输出句柄
/// * `rx` - 从调度器接收消息的通道接收器
///
/// # 返回
///
/// 返回 `Result<()>`，表示操作是否成功
///
/// # 错误
///
/// 如果写入或刷新失败，将返回错误
pub async fn send_data_frontend(
    mut stdout: Stdout,
    mut rx: mpsc::UnboundedReceiver<String>,
) -> Result<()> {
    while let Some(message) = rx.recv().await {
        // 发送数据到vscode
        stdout.write_all(message.as_bytes()).await?;
        stdout.flush().await?;
        info!("已发送: {}", message);
    }
    Ok(())
}

/// 从前端（VSCode）接收数据的异步任务。
///
/// 这个函数读取标准输入，按照 LSP 协议解析消息头和消息体，
/// 然后将解析后的 JSON 消息传递给调度器进行处理。
///
/// # 参数
///
/// * `stdin` - 标准输入缓冲读取器
/// * `dispatcher` - 调度器实例，用于处理接收到的消息
///
/// # 返回
///
/// 返回 `Result<()>`，表示操作是否成功
///
/// # 错误
///
/// 如果读取、解析或处理消息失败，将返回错误
pub async fn receive_data_frontend(
    stdin: BufReader<Stdin>,
    dispatcher: Arc<Dispatcher>,
    semaphore: Arc<Semaphore>,
) -> Result<()> {
    let mut reader = stdin;

    loop {
        // 1. 读取 header
        let mut content_length = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            let line = line.trim();

            if line.is_empty() {
                break;
            }

            if let Some(cl) = line.strip_prefix("Content-Length:") {
                content_length = Some(
                    cl.trim()
                        .parse::<usize>()
                        .context("Content-Length 解析失败")?,
                );
            }
        }

        let content_length = match content_length {
            Some(len) => len,
            None => continue,
        };

        // 2. 读取 body
        let mut body_buf = vec![0u8; content_length];
        reader.read_exact(&mut body_buf).await?;
        // let body_str = String::from_utf8(body_buf).context("UTF-8 解码失败")?;

        // 3. 解析 JSON
        // let json_body: Value = serde_json::from_str(&body_str).context("JSON 解析失败")?;
        let json_body: Value = serde_json::from_slice(&body_buf).context("JSON 解析失败")?;

        let permit = semaphore.clone().acquire_owned().await?;
        let _ = permit;

        // 5. 并发处理
        let dispatcher = dispatcher.clone();
        tokio::spawn(async move {
            if let Err(e) = dispatcher.handle_from_frontend(json_body).await {
                error!("前端消息处理失败: {:?}", e);
            }
        });
    }
}