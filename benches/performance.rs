use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::{json, Value};
use std::hint::black_box;
use std::sync::Arc;
use tokio::sync::mpsc;

use codefuse::dispatcher::Dispatcher;

fn bench_json_parsing(c: &mut Criterion) {
    println!("Starting bench_json_parsing");
    let body_buf = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"textDocument/hover\",\"params\":{\"textDocument\":{\"uri\":\"file:///test.cpp\"},\"position\":{\"line\":10,\"character\":5}}}";

    c.bench_function("json_from_slice_zero_copy", |b| {
        b.iter(|| {
            let _: Value = serde_json::from_slice(black_box(body_buf)).unwrap();
        });
    });

    let body_str = String::from_utf8(body_buf.to_vec()).unwrap();
    c.bench_function("json_from_str_string_copy", |b| {
        b.iter(|| {
            let _: Value = serde_json::from_str(black_box(&body_str)).unwrap();
        });
    });
}

fn bench_dispatcher_handle(c: &mut Criterion) {
    println!("Starting bench_dispatcher_handle");
    // 跳过async测试，使用同步模拟
    let (backend_tx, _) = mpsc::unbounded_channel::<String>();
    let (frontend_tx, _) = mpsc::unbounded_channel::<String>();
    let dispatcher = Arc::new(Dispatcher::new(backend_tx, frontend_tx));

    let rpc = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "textDocument/hover",
        "params": {
            "textDocument": {"uri": "file:///test.cpp"},
            "position": {"line": 10, "character": 5}
        }
    });

    c.bench_function("handle_from_frontend_sync_sim", |b| {
        b.iter(|| {
            // 同步模拟：只测试格式化部分
            black_box(Dispatcher::format_notification_or_request(&rpc));
        });
    });

    let response_rpc = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "contents": {"kind": "plaintext", "value": "hover content"}
        }
    });

    c.bench_function("handle_from_backend_sync_sim", |b| {
        b.iter(|| {
            black_box(Dispatcher::format_result(response_rpc.clone()));
        });
    });
}

fn bench_message_formatting(c: &mut Criterion) {
    println!("Starting bench_message_formatting");
    let result = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "contents": {"kind": "plaintext", "value": "test content"}
        }
    });

    c.bench_function("format_lsp_message", |b| {
        b.iter(|| {
            black_box(Dispatcher::format_lsp_message(&result).unwrap());
        });
    });
}

criterion_group!(
    benches,
    bench_json_parsing,
    bench_dispatcher_handle,
    bench_message_formatting
);
criterion_main!(benches);