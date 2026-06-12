use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::lsp_client::LspClient;
use crate::{ResolutionSource, ResolveContext, ResolvedSymbol, SymbolKind, TypeResolver};

pub struct JavaLspResolver {
    client: Arc<Mutex<LspClient>>,
    workspace_root: PathBuf,
    cache: tokio::sync::RwLock<HashMap<String, Vec<ResolvedSymbol>>>,
}

impl JavaLspResolver {
    pub async fn new(workspace_root: &Path) -> Result<Self> {
        let jdtls_cmd = find_jdtls()?;
        tracing::info!(command = %jdtls_cmd, "Starting jdtls");

        let data_dir = std::env::temp_dir().join(format!(
            "specgraphen-jdt-{}",
            workspace_root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));

        let mut client = LspClient::spawn(
            &jdtls_cmd,
            &["-data", &data_dir.to_string_lossy()],
            workspace_root,
        )
        .await?;

        client.initialize(workspace_root).await?;

        // Give jdtls time to index
        tracing::info!("Waiting for jdtls to index workspace...");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            workspace_root: workspace_root.to_path_buf(),
            cache: tokio::sync::RwLock::new(HashMap::new()),
        })
    }

    fn file_uri(&self, file: &str) -> String {
        let abs_path = if Path::new(file).is_absolute() {
            PathBuf::from(file)
        } else {
            self.workspace_root.join(file)
        };
        path_to_uri(&abs_path)
    }
}

#[async_trait]
impl TypeResolver for JavaLspResolver {
    async fn resolve_type(&self, type_name: &str, ctx: &ResolveContext) -> Vec<ResolvedSymbol> {
        let cache_key = format!("type:{}:{}:{}", type_name, ctx.file, ctx.line);

        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                return cached.clone();
            }
        }

        let file = self.file_uri(&ctx.file);
        let mut client = self.client.lock().await;

        let results = match client
            .definition(&file, ctx.line.saturating_sub(1), ctx.column)
            .await
        {
            Ok(locations) => locations
                .into_iter()
                .filter_map(|loc| {
                    let fqn = uri_to_fqn(&loc.uri, &self.workspace_root);
                    fqn.map(|fqn| ResolvedSymbol {
                        fqn,
                        kind: SymbolKind::Class,
                        source: ResolutionSource::Lsp,
                        file: Some(uri_to_path(&loc.uri)),
                        line: Some(loc.start_line + 1),
                    })
                })
                .collect(),
            Err(e) => {
                tracing::trace!("LSP resolve_type failed for {type_name}: {e}");
                Vec::new()
            }
        };

        // Cache result
        {
            let mut cache = self.cache.write().await;
            cache.insert(cache_key, results.clone());
        }

        results
    }

    async fn resolve_method_call(
        &self,
        method: &str,
        object: Option<&str>,
        ctx: &ResolveContext,
    ) -> Vec<ResolvedSymbol> {
        let cache_key = format!(
            "call:{}:{}:{}:{}",
            method,
            object.unwrap_or(""),
            ctx.file,
            ctx.line
        );

        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                return cached.clone();
            }
        }

        let file = self.file_uri(&ctx.file);
        let mut client = self.client.lock().await;

        let results = match client
            .definition(&file, ctx.line.saturating_sub(1), ctx.column)
            .await
        {
            Ok(locations) => locations
                .into_iter()
                .filter_map(|loc| {
                    let fqn = uri_to_fqn(&loc.uri, &self.workspace_root);
                    fqn.map(|fqn| ResolvedSymbol {
                        fqn,
                        kind: SymbolKind::Method,
                        source: ResolutionSource::Lsp,
                        file: Some(uri_to_path(&loc.uri)),
                        line: Some(loc.start_line + 1),
                    })
                })
                .collect(),
            Err(e) => {
                tracing::trace!("LSP resolve_method_call failed for {method}: {e}");
                Vec::new()
            }
        };

        {
            let mut cache = self.cache.write().await;
            cache.insert(cache_key, results.clone());
        }

        results
    }

    async fn find_references(&self, _fqn: &str, ctx: &ResolveContext) -> Vec<ResolvedSymbol> {
        let file = self.file_uri(&ctx.file);
        let mut client = self.client.lock().await;

        match client
            .references(&file, ctx.line.saturating_sub(1), ctx.column)
            .await
        {
            Ok(locations) => locations
                .into_iter()
                .map(|loc| ResolvedSymbol {
                    fqn: uri_to_path(&loc.uri),
                    kind: SymbolKind::Unknown,
                    source: ResolutionSource::Lsp,
                    file: Some(uri_to_path(&loc.uri)),
                    line: Some(loc.start_line + 1),
                })
                .collect(),
            Err(e) => {
                tracing::trace!("LSP find_references failed: {e}");
                Vec::new()
            }
        }
    }

    fn name(&self) -> &str {
        "jdtls"
    }
}

impl Drop for JavaLspResolver {
    fn drop(&mut self) {
        // shutdown is async, best-effort via Drop
    }
}

fn find_jdtls() -> Result<String> {
    // Check common locations
    let candidates = ["jdtls", "jdt-language-server", "eclipse.jdt.ls"];

    for cmd in &candidates {
        if which_exists(cmd) {
            return Ok(cmd.to_string());
        }
    }

    // Check JDTLS_HOME environment variable
    if let Ok(home) = std::env::var("JDTLS_HOME") {
        let launcher = find_launcher_jar(&home);
        if let Some(jar) = launcher {
            return Ok(format!(
                "java -Declipse.application=org.eclipse.jdt.ls.core.id1 \
                 -Dosgi.bundles.defaultStartLevel=4 \
                 -Declipse.product=org.eclipse.jdt.ls.core.product \
                 -Xmx1G --add-modules=ALL-SYSTEM -jar {jar}"
            ));
        }
    }

    anyhow::bail!(
        "jdtls not found. Install it via your package manager or set JDTLS_HOME. \
         On macOS: brew install jdtls"
    )
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn find_launcher_jar(jdtls_home: &str) -> Option<String> {
    let plugins_dir = Path::new(jdtls_home).join("plugins");
    if let Ok(entries) = std::fs::read_dir(plugins_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("org.eclipse.equinox.launcher_") && name.ends_with(".jar") {
                return Some(entry.path().to_string_lossy().to_string());
            }
        }
    }
    None
}

fn path_to_uri(path: &Path) -> String {
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

fn uri_to_path(uri: &str) -> String {
    let raw = uri.strip_prefix("file://").unwrap_or(uri);
    percent_decode(raw)
}

fn percent_decode(s: &str) -> String {
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

fn uri_to_fqn(uri: &str, workspace_root: &Path) -> Option<String> {
    let path = uri_to_path(uri);
    let rel = Path::new(&path)
        .strip_prefix(workspace_root)
        .ok()?
        .to_string_lossy()
        .to_string();

    // Convert file path to FQN: src/main/java/com/example/Foo.java → com.example.Foo
    let fqn = rel.trim_end_matches(".java").replace(['/', '\\'], ".");

    // Strip common source root prefixes
    let fqn = fqn
        .strip_prefix("src.main.java.")
        .or_else(|| fqn.strip_prefix("src."))
        .or_else(|| fqn.strip_prefix("webapp.wssrc."))
        .unwrap_or(&fqn)
        .to_string();

    Some(fqn)
}
