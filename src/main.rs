mod clangd_client;
mod dispatcher;

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::clangd_client::ClangdClient;
use crate::dispatcher::Dispatcher;
use serde_json;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Stdin, Stdout};
use tokio::process::{ChildStdin, ChildStdout};
use tower_lsp::lsp_types::{InitializeResult, ServerInfo};

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
                frontend_sender.send(message).await.unwrap();
                Ok(())
            }
        })
        .await;
}

async fn send_data_backend(mut stdin: ChildStdin, mut rx: mpsc::Receiver<String>) -> Result<()> {
    while let Some(message) = rx.recv().await {
        // 发送数据到外部程序
        stdin.write_all(message.as_bytes()).await?;
        stdin.flush().await?;
        eprintln!("已发送: {}", message);
    }
    Ok(())
}

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
                eprintln!("处理失败: {:?}", e);
            }
        });
    }
}

async fn send_data_frontend(mut stdout: Stdout, mut rx: mpsc::Receiver<String>) -> Result<()> {
    while let Some(message) = rx.recv().await {
        // 发送数据到vscode
        stdout.write_all(message.as_bytes()).await?;
        stdout.flush().await?;
        eprintln!("已发送: {}", message);
    }
    Ok(())
}

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
                eprintln!("前端消息处理失败: {:?}", e);
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ClangdClient {
        stdin,
        stdout,
        id_counter: _,
    } = ClangdClient::spawn().await;

    // 读取 VSCode 请求
    let reader = BufReader::new(tokio::io::stdin());
    let writer = tokio::io::stdout();

    let (backend_tx, backend_rx) = mpsc::channel::<String>(1000);
    let (frontend_tx, frontend_rx) = mpsc::channel::<String>(1000);

    let send_backend_handle = tokio::spawn(send_data_backend(stdin, backend_rx));
    let send_frontend_handle = tokio::spawn(send_data_frontend(writer, frontend_rx));

    let dispatcher = Arc::new(Dispatcher::new(backend_tx, frontend_tx));

    let recv_backend_handle = tokio::spawn(receive_data_backend(stdout, Arc::clone(&dispatcher)));
    let recv_frontend_handle = tokio::spawn(receive_data_frontend(reader, Arc::clone(&dispatcher)));

    setup_handlers(Arc::clone(&dispatcher)).await;

    tokio::select! {
        result = send_backend_handle => {
            if let Err(e) = result {
                eprintln!("后端发送任务失败: {:?}", e);
            }
        },
        result = send_frontend_handle => {
            if let Err(e) = result {
                eprintln!("前端发送任务失败: {:?}", e);
            }
        },
        result = recv_backend_handle => {
            if let Err(e) = result {
                eprintln!("后端接收任务失败: {:?}", e);
            }
        },
        result = recv_frontend_handle => {
            if let Err(e) = result {
                eprintln!("前端接收任务失败: {:?}", e);
            }
        }
    }

    Ok(())
}
