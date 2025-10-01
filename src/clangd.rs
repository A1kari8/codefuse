use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::time::{timeout, Duration};
use std::sync::atomic::{AtomicU32, Ordering};

pub struct ClangdSession {
    stdin: ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    id: AtomicU32,
}

impl ClangdSession {
    pub(crate) async fn new() -> Result<Self, std::io::Error> {
        let mut child = Command::new("clangd")
            .arg("--log=verbose")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?; // 用 ? 传播错误

        let stdin = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to open stdin")
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to open stdout")
        })?;

        let reader = BufReader::new(stdout);

        Ok(ClangdSession { stdin, reader, id: AtomicU32::new(5) })
    }

    pub(crate) async fn send_hover(&mut self, file_uri: &str, line: u32, character: u32) -> String {
        let id = self.id.fetch_add(1, Ordering::SeqCst);
        let payload = format!(
            r#"{{
            "jsonrpc": "2.0",
            "id": {},
            "method": "textDocument/hover",
            "params": {{
                "textDocument": {{ "uri": "{}" }},
                "position": {{ "line": {}, "character": {} }}
            }}
        }}"#,
            id, file_uri, line, character
        );

        let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        match self.send_request(&request, id).await {
            Ok(response) => response,
            Err(e) => format!("error: {}", e),
        }
    }

    pub(crate) async fn send_completion(&mut self, file_uri: &str, line: u32, character: u32) -> String {
        let id = self.id.fetch_add(1, Ordering::SeqCst);
        let payload = format!(
            r#"{{
            "jsonrpc": "2.0",
            "id": {},
            "method": "textDocument/completion",
            "params": {{
                "textDocument": {{ "uri": "{}" }},
                "position": {{ "line": {}, "character": {} }}
            }}
        }}"#,
            id, file_uri, line, character
        );

        let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        match self.send_request(&request, id).await {
            Ok(response) => response,
            Err(e) => format!("error: {}", e),
        }
    }

    pub(crate) async fn send_semantic_tokens(&mut self, file_uri: &str) -> String {
        let id = self.id.fetch_add(1, Ordering::SeqCst);
        let payload = format!(
            r#"{{
            "jsonrpc": "2.0",
            "id": {},
            "method": "textDocument/semanticTokens/full",
            "params": {{
                "textDocument": {{ "uri": "{}" }}
            }}
        }}"#,
            id, file_uri
        );

        let request = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);

        match self.send_request(&request, id).await {
            Ok(response) => response,
            Err(e) => format!("error: {}", e),
        }
    }

    pub async fn read_response(&mut self) -> Result<serde_json::Value, std::io::Error> {
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = self.reader.read_line(&mut line).await?;
            if bytes == 0 {
                break;
            }
            if line.trim().is_empty() {
                break;
            }
            headers.push_str(&line);
        }

        // 解析 Content-Length
        let content_length = headers
            .lines()
            .find(|line| line.to_lowercase().starts_with("content-length:"))
            .and_then(|line| line.split(':').nth(1))
            .and_then(|val| val.trim().parse::<usize>().ok())
            .unwrap_or(0);

        // 读取 body
        let mut body = vec![0u8; content_length];
        self.reader.read_exact(&mut body).await?;
        let json: serde_json::Value = serde_json::from_slice(&body)?;
        Ok(json)
    }

    pub(crate) async fn send_request(&mut self, request: &str, expected_id: u32) -> Result<String, std::io::Error> {
        self.stdin.write_all(request.as_bytes()).await?;
        self.stdin.flush().await?;
        loop {
            let mut header_line = String::new();
            match timeout(Duration::from_secs(5), (&mut self.reader).read_line(&mut header_line)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "read timeout")),
            }
            if !header_line.starts_with("Content-Length: ") {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Invalid response header",
                ));
            }
            let length_str = header_line.trim_start_matches("Content-Length: ").trim();
            let length: usize = length_str
                .parse()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Parse Content-Length"))?;
            let mut empty_line = String::new();
            match timeout(Duration::from_secs(5), (&mut self.reader).read_line(&mut empty_line)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "read timeout")),
            }
            let mut buffer = vec![0; length];
            match timeout(Duration::from_secs(5), self.reader.read_exact(&mut buffer)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "read timeout")),
            }
            let response = String::from_utf8_lossy(&buffer).to_string();
            // Parse JSON to check id
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&response) {
                if let Some(id) = value.get("id") {
                    if id.as_u64() == Some(expected_id as u64) {
                        return Ok(response);
                    }
                }
                // If no id or id doesn't match, continue loop (skip notifications or other responses)
            } else {
                // If not valid JSON, perhaps log and continue, but for now return error
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "Invalid JSON response"));
            }
        }
    }

    pub(crate) async fn send_notification(&mut self, notification: &str) -> Result<(), std::io::Error> {
        self.stdin.write_all(notification.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}
