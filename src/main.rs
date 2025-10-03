//! # MyLSP - LSP 代理服务器
//!
//! 这个模块实现了一个LSP (Language Server Protocol) 代理服务器，用于在 VSCode 前端和 clangd 后端语言服务器之间进行通信和数据转发。
//! 它允许对 LSP 消息进行拦截、修改和增强处理。
//!
//! ## 主要组件
//!
//! - `clangd_client`: 负责启动和管理 clangd 进程
//! - `dispatcher`: 负责消息的分发和处理逻辑
//! - `main`: 主程序入口，设置异步任务和消息循环

mod clangd_client;
mod dispatcher;

use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::clangd_client::ClangdClient;
use crate::dispatcher::Dispatcher;
use serde_json;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Stdin, Stdout};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tower_lsp::lsp_types::{InitializeResult, ServerInfo};

/// 设置处理器函数，为特定的 LSP 方法注册处理逻辑。
///
/// 这个函数用于注册从后端（clangd）接收到的消息的处理函数。
/// 目前注册了 `initialize` 方法的处理器，用于修改初始化响应。
///
/// # 参数
///
/// * `dispatcher` - 调度器实例，用于注册处理器
///
/// # 示例
///
/// ```rust
/// setup_handlers(dispatcher.clone()).await;
/// ```
pub async fn setup_handlers(dispatcher: Arc<Dispatcher>) {
    // reg.register("textDocument/hover", move |params,clangd,vscode_out| {
    //     async move {
    //         let mut client = clangd.lock().await;
    //         let raw_result = client.send_request("textDocument/hover", params).await?;
    //
    //         // Step 1: 转成 Hover 类型
    //         let mut hover: Hover = serde_json::from_value(raw_result).ok()?;
    //
    //         // Step 2: 编辑 Hover 内容
    //         match &mut hover.contents {
    //             HoverContents::Scalar(MarkedString::String(s)) => {
    //                 s.push_str("\n\n---\nEnhanced by proxy");
    //             }
    //             HoverContents::Scalar(MarkedString::LanguageString(ls)) => {
    //                 ls.value.push_str("\n\n// Enhanced by proxy");
    //             }
    //             HoverContents::Array(arr) => {
    //                 arr.push(MarkedString::String("Enhanced by proxy".into()));
    //             }
    //             _ => {}
    //         }
    //
    //         // Step 3: 转回 JSON
    //         let edited = serde_json::to_value(hover).ok()?;
    //         Some(json!({ "result": edited }))
    //     }
    // }).await;

    dispatcher
        .register_from_backend("initialize", |rpc, frontend_sender| {
            async move {
                let mut raw_rpc = rpc.clone();
                // Step 1: 转成 tower-lsp
                let raw_result = rpc
                    .get("result")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Missing result field"))?;
                let mut init_result: InitializeResult = serde_json::from_value(raw_result)?;

                init_result.server_info = Some(ServerInfo {
                    name: "codefuse".into(),
                    version: Some("0.1.0".into()),
                });

                let edited = serde_json::to_value(init_result)?;

                if let Some(obj) = raw_rpc.as_object_mut() {
                    obj.insert("result".to_string(), edited); // 修改字段
                }

                // Step 3: 转回 JSON
                let message = Dispatcher::format_lsp_message(&raw_rpc)?;
                frontend_sender.send(message)?;
                Ok(())
            }
        })
        .await;
}

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
async fn send_data_backend(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<String>) -> Result<()> {
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
async fn receive_data_backend(
    stdout: BufReader<ChildStdout>,
    dispatcher: Arc<Dispatcher>,
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
        let body_str = String::from_utf8(body_buf).context("UTF-8 解码失败")?;

        // 3. 解析 JSON
        let json_body: Value = serde_json::from_str(&body_str).context("JSON 解析失败")?;

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
async fn send_data_frontend(mut stdout: Stdout, mut rx: mpsc::UnboundedReceiver<String>) -> Result<()> {
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
async fn receive_data_frontend(stdin: BufReader<Stdin>, dispatcher: Arc<Dispatcher>) -> Result<()> {
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
        let body_str = String::from_utf8(body_buf).context("UTF-8 解码失败")?;

        // 3. 解析 JSON
        let json_body: Value = serde_json::from_str(&body_str).context("JSON 解析失败")?;

        // 5. 并发处理
        let dispatcher = dispatcher.clone();
        tokio::spawn(async move {
            if let Err(e) = dispatcher.handle_from_frontend(json_body).await {
                error!("前端消息处理失败: {:?}", e);
            }
        });
    }
}

async fn pipe_clangd_stderr(stderr: BufReader<ChildStderr>) {
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

/// 主函数，程序的入口点。
///
/// 这个函数设置了整个 LSP 代理服务器的架构：
/// - 启动 clangd 进程
/// - 创建消息通道
/// - 启动发送和接收数据的异步任务
/// - 设置消息处理器
/// - 等待任一任务完成
///
/// # 返回
///
/// 返回 `Result<(), Box<dyn std::error::Error>>`，表示程序是否成功运行
///
/// # 错误
///
/// 如果任何异步任务失败，将返回错误
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .write_style(env_logger::WriteStyle::Always)
        .target(env_logger::Target::Stderr) // 写入 stderr，避免污染 stdout
        .init();

    info!("Starting LSP proxy server...");

    let ClangdClient {
        stdin,
        stdout,
        stderr,
        id_counter: _,
    } = ClangdClient::spawn().await;

    tokio::spawn(pipe_clangd_stderr(stderr));

    // 读取 VSCode 请求
    let reader = BufReader::new(tokio::io::stdin());
    let writer = tokio::io::stdout();

    let (backend_tx, backend_rx) = mpsc::unbounded_channel::<String>();
    let (frontend_tx, frontend_rx) = mpsc::unbounded_channel::<String>();

    let send_backend_handle = tokio::spawn(send_data_backend(stdin, backend_rx));
    let send_frontend_handle = tokio::spawn(send_data_frontend(writer, frontend_rx));

    let dispatcher = Arc::new(Dispatcher::new(backend_tx, frontend_tx));

    let recv_backend_handle = tokio::spawn(receive_data_backend(stdout, Arc::clone(&dispatcher)));
    let recv_frontend_handle = tokio::spawn(receive_data_frontend(reader, Arc::clone(&dispatcher)));

    setup_handlers(Arc::clone(&dispatcher)).await;

    tokio::select! {
        result = send_backend_handle => {
            if let Err(e) = result {
                error!("后端发送任务失败: {:?}", e);
            }
        },
        result = send_frontend_handle => {
            if let Err(e) = result {
                error!("前端发送任务失败: {:?}", e);
            }
        },
        result = recv_backend_handle => {
            if let Err(e) = result {
                error!("后端接收任务失败: {:?}", e);
            }
        },
        result = recv_frontend_handle => {
            if let Err(e) = result {
                error!("前端接收任务失败: {:?}", e);
            }
        }
    }

    Ok(())
}
