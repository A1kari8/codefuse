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

mod lsp_backend;
mod dispatcher;
mod handlers;
mod tasks;

use crate::lsp_backend::{LspBackend, pipe_lsp_backend_stderr};
use crate::dispatcher::Dispatcher;
use crate::handlers::setup_handlers;
use crate::tasks::*;
use anyhow::Result;
use chrono::Local;
use log::{error, info};
use std::io::Write;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::sync::{Semaphore, mpsc};

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
async fn main() -> Result<()> {
    env_logger::Builder::new()
        .format(|buf, record| {
            let style = buf.default_level_style(record.level());
            write!(buf, "[{} ", Local::now().format("%H:%M:%S"))?;
            write!(buf, "{}", style)?;
            write!(buf, "{} \x1b[0m", record.level())?;
            writeln!(
                buf,
                "{}] {}",
                record.module_path().unwrap_or(env!("CARGO_PKG_NAME")),
                record.args()
            )
        })
        .filter_level(log::LevelFilter::Info)
        .write_style(env_logger::WriteStyle::Auto)
        .target(env_logger::Target::Stderr) // 写入 stderr，避免污染 stdout
        .init();

    info!("Starting LSP proxy server...");

    let LspBackend {
        stdin,
        stdout,
        stderr,
        id_counter: _,
    } = LspBackend::spawn("clangd").await;

    tokio::spawn(pipe_lsp_backend_stderr(stderr));

    // 读取 VSCode 请求
    let reader = BufReader::new(tokio::io::stdin());
    let writer = tokio::io::stdout();

    let (backend_tx, backend_rx) = mpsc::unbounded_channel::<String>();
    let (frontend_tx, frontend_rx) = mpsc::unbounded_channel::<String>();

    let send_backend_handle = tokio::spawn(send_data_backend(stdin, backend_rx));
    let send_frontend_handle = tokio::spawn(send_data_frontend(writer, frontend_rx));

    let dispatcher = Arc::new(Dispatcher::new(backend_tx, frontend_tx));

    let semaphore = Arc::new(Semaphore::new(15)); // 限制最多 10 个并发任务

    let recv_backend_handle = tokio::spawn(receive_data_backend(
        stdout,
        Arc::clone(&dispatcher),
        Arc::clone(&semaphore),
    ));
    let recv_frontend_handle = tokio::spawn(receive_data_frontend(
        reader,
        Arc::clone(&dispatcher),
        Arc::clone(&semaphore),
    ));

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
