use std::sync::atomic::AtomicU64;
use tokio::io::BufReader;
use tokio::process::{ChildStdin, ChildStdout, Command};

pub struct ClangdClient {
    pub stdin: ChildStdin,
    pub stdout: BufReader<ChildStdout>,
    pub id_counter: AtomicU64,
}

impl ClangdClient {
    pub async fn spawn() -> Self {
        let mut child = Command::new("clangd")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("Failed to start clangd");

        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());

        Self {
            stdin,
            stdout,
            id_counter: AtomicU64::new(1),
        }
    }
}
