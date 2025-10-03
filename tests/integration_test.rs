use std::fs;
use std::process::Stdio;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

use codefuse::dispatcher::Dispatcher;
use serde_json::json;

#[tokio::test]
async fn test_hover_end_to_end() {
    // 创建临时 C++ 文件
    let temp_file = NamedTempFile::new().unwrap();
    let file_path = temp_file.path().to_str().unwrap().to_string();
    let file_uri = format!("file://{}", file_path);

    let cpp_content = r#"
#include <iostream>

int main() {
    std::cout << "Hello, world!" << std::endl;
    return 0;
}
"#;
    fs::write(&file_path, cpp_content).unwrap();

    // 启动 clangd 进程
    let mut clangd = Command::new("clangd")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start clangd");

    let clangd_stdin = clangd.stdin.take().unwrap();
    let clangd_stdout = BufReader::new(clangd.stdout.take().unwrap());

    // 创建通道
    let (backend_tx, mut backend_rx) = mpsc::unbounded_channel::<String>();
    let (frontend_tx, mut frontend_rx) = mpsc::unbounded_channel::<String>();

    let dispatcher = Arc::new(Dispatcher::new(backend_tx, frontend_tx));

    // 启动发送到 clangd 的任务
    let send_handle = tokio::spawn(async move {
        let mut stdin = clangd_stdin;
        while let Some(msg) = backend_rx.recv().await {
            stdin.write_all(msg.as_bytes()).await.unwrap();
            stdin.flush().await.unwrap();
        }
    });

    // 启动从 clangd 接收并转发回前端的任务
    let dispatcher_clone = Arc::clone(&dispatcher);
    let recv_handle = tokio::spawn(async move {
        let mut reader = clangd_stdout;
        loop {
            // 读取 LSP 消息头
            let mut content_length = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap() == 0 {
                    return; // EOF
                }
                let line = line.trim();
                if line.is_empty() {
                    break;
                }
                if let Some(cl) = line.strip_prefix("Content-Length:") {
                    content_length = Some(cl.trim().parse::<usize>().unwrap());
                }
            }

            if let Some(len) = content_length {
                // 读取消息体
                let mut body_buf = vec![0u8; len];
                reader.read_exact(&mut body_buf).await.unwrap();
                let json_body: serde_json::Value = serde_json::from_slice(&body_buf).unwrap();

                // 通过 Dispatcher 处理后端响应
                dispatcher_clone.handle_from_backend(json_body).await.unwrap();
            }
        }
    });

    // 发送 initialize 请求
    let root_uri = format!("file://{}", temp_file.path().parent().unwrap().to_str().unwrap());
    let rpc = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": root_uri,
            "capabilities": {}
        }
    });
    dispatcher.handle_from_frontend(rpc).await.unwrap();
    let _ = frontend_rx.recv().await.unwrap(); // 等待 initialize 响应

    // 发送 didOpen 请求
    let rpc = json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri,
                "languageId": "cpp",
                "version": 1,
                "text": cpp_content
            }
        }
    });
    dispatcher.handle_from_frontend(rpc).await.unwrap();

    // 发送 hover 请求
    let rpc = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/hover",
        "params": {
            "textDocument": {"uri": file_uri},
            "position": {"line": 3, "character": 5}
        }
    });

    let start = Instant::now();
    dispatcher.handle_from_frontend(rpc).await.unwrap();

    // 等待 hover 响应
    let response = frontend_rx.recv().await.unwrap();
    let elapsed = start.elapsed();

    println!("Hover end-to-end roundtrip time: {:?}", elapsed);
    println!("Hover response length: {}", response.len());

    // 清理
    send_handle.abort();
    recv_handle.abort();
    clangd.kill().await.unwrap();
    drop(temp_file); // 删除临时文件

    // 合格标准：hover < 50 ms
    assert!(elapsed < Duration::from_millis(50), "Hover roundtrip should be < 50ms, got {:?}", elapsed);
}