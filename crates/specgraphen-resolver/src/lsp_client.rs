use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub struct LspClient {
    process: Child,
    stdin: tokio::io::BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    request_id: AtomicU64,
    initialized: bool,
}

impl LspClient {
    pub async fn spawn(command: &str, args: &[&str], workspace_root: &Path) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .current_dir(workspace_root)
            .spawn()
            .with_context(|| format!("Failed to spawn LSP server: {command}"))?;

        let stdin = child.stdin.take().context("No stdin")?;
        let stdout = child.stdout.take().context("No stdout")?;

        Ok(Self {
            process: child,
            stdin: tokio::io::BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            request_id: AtomicU64::new(1),
            initialized: false,
        })
    }

    pub async fn initialize(
        &mut self,
        workspace_root: &Path,
        initialization_options: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let root_uri = path_to_file_uri(workspace_root);

        let mut params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "hover": { "contentFormat": ["plaintext"] },
                    "definition": { "linkSupport": false },
                    "references": {},
                    "callHierarchy": {}
                },
                "workspace": {
                    "symbol": { "resolveSupport": { "properties": [] } }
                }
            },
            "workspaceFolders": [{
                "uri": root_uri,
                "name": workspace_root.file_name().unwrap_or_default().to_string_lossy()
            }]
        });

        if let Some(options) = initialization_options {
            params["initializationOptions"] = options;
        }

        let result = self.send_request("initialize", params).await?;
        self.send_notification("initialized", serde_json::json!({}))
            .await?;
        self.initialized = true;

        tracing::info!("LSP server initialized");
        Ok(result)
    }

    /// Wait until the server reports readiness via a `language/status` notification
    /// (jdtls sends `type: "ServiceReady"` once language services are available).
    /// Returns `false` if the timeout elapsed without seeing the notification.
    pub async fn wait_for_ready(&mut self, timeout: std::time::Duration) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                return Ok(false);
            };
            let message = match tokio::time::timeout(remaining, self.read_message()).await {
                Err(_) => return Ok(false),
                Ok(Err(e)) => return Err(e),
                Ok(Ok(m)) => m,
            };
            if message["method"].as_str() == Some("language/status") {
                match message["params"]["type"].as_str().unwrap_or("") {
                    "ServiceReady" => return Ok(true),
                    "Error" => {
                        tracing::warn!(
                            message = %message["params"]["message"],
                            "LSP server reported an error during startup"
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    pub async fn did_open(&mut self, uri: &str, language_id: &str, text: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text
                }
            }),
        )
        .await
    }

    pub async fn hover(&mut self, file: &str, line: u32, col: u32) -> Result<Option<String>> {
        let uri = file;
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col }
        });

        let result = self.send_request("textDocument/hover", params).await?;
        if result.is_null() {
            return Ok(None);
        }

        let contents = &result["contents"];
        let text = if let Some(s) = contents.as_str() {
            s.to_string()
        } else if let Some(s) = contents["value"].as_str() {
            s.to_string()
        } else {
            contents.to_string()
        };

        Ok(Some(text))
    }

    pub async fn definition(
        &mut self,
        file: &str,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>> {
        let uri = file;
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col }
        });

        let result = self.send_request("textDocument/definition", params).await?;
        parse_locations(&result)
    }

    pub async fn references(
        &mut self,
        file: &str,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>> {
        let uri = file;
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col },
            "context": { "includeDeclaration": false }
        });

        let result = self.send_request("textDocument/references", params).await?;
        parse_locations(&result)
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        if self.initialized {
            let _ = self.send_request("shutdown", serde_json::Value::Null).await;
            self.send_notification("exit", serde_json::Value::Null)
                .await?;
        }
        let _ = self.process.kill().await;
        Ok(())
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.write_message(&message).await?;
        self.read_response(id).await
    }

    async fn send_notification(&mut self, method: &str, params: serde_json::Value) -> Result<()> {
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        self.write_message(&message).await
    }

    async fn write_message(&mut self, message: &serde_json::Value) -> Result<()> {
        let body = serde_json::to_string(message)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(body.as_bytes()).await?;
        self.stdin.flush().await?;

        Ok(())
    }

    async fn read_message(&mut self) -> Result<serde_json::Value> {
        // Read Content-Length header
        let mut header = String::new();
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).await?;
            if line.trim().is_empty() {
                break;
            }
            header.push_str(&line);
        }

        let content_length: usize = header
            .lines()
            .find_map(|l| {
                l.strip_prefix("Content-Length: ")
                    .and_then(|v| v.trim().parse().ok())
            })
            .context("Missing Content-Length header")?;

        let mut body = vec![0u8; content_length];
        self.stdout.read_exact(&mut body).await?;

        Ok(serde_json::from_slice(&body)?)
    }

    async fn read_response(&mut self, expected_id: u64) -> Result<serde_json::Value> {
        loop {
            let response = self.read_message().await?;

            // Skip notifications (no id field)
            if response.get("id").is_none() {
                continue;
            }

            if response["id"].as_u64() == Some(expected_id) {
                if let Some(error) = response.get("error") {
                    anyhow::bail!("LSP error: {}", error);
                }
                return Ok(response["result"].clone());
            }
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Best-effort kill
        let _ = self.process.start_kill();
    }
}

#[derive(Debug, Clone)]
pub struct LspLocation {
    pub uri: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

fn path_to_file_uri(path: &Path) -> String {
    let abs = path.to_string_lossy();
    let encoded: String = abs
        .bytes()
        .map(|b| match b {
            b'/' | b'.' | b'-' | b'_' | b'~' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b':' => {
                format!("{}", b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect();
    format!("file://{encoded}")
}

fn parse_locations(value: &serde_json::Value) -> Result<Vec<LspLocation>> {
    if value.is_null() {
        return Ok(Vec::new());
    }

    let locations = if value.is_array() {
        value.as_array().unwrap().clone()
    } else {
        vec![value.clone()]
    };

    Ok(locations
        .iter()
        .filter_map(|loc| {
            let uri = loc["uri"].as_str()?.to_string();
            let range = &loc["range"];
            Some(LspLocation {
                uri,
                start_line: range["start"]["line"].as_u64()? as u32,
                start_col: range["start"]["character"].as_u64()? as u32,
                end_line: range["end"]["line"].as_u64()? as u32,
                end_col: range["end"]["character"].as_u64()? as u32,
            })
        })
        .collect())
}
