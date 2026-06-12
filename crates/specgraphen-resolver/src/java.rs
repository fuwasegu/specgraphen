use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::lsp_client::LspClient;
use crate::{ResolutionSource, ResolveContext, ResolvedSymbol, SymbolKind, TypeResolver};

#[derive(Debug, Clone)]
pub struct JavaLspOptions {
    /// How long to wait for jdtls to report `ServiceReady` before proceeding anyway.
    pub init_timeout: Duration,
    /// Source roots relative to the workspace root (e.g. `src/main/java`).
    /// When empty, roots are auto-detected from package declarations.
    pub source_roots: Vec<String>,
    /// Source file encoding label (e.g. `shift_jis`, `windows-31j`).
    /// When unset, encoding is detected per file (UTF-8 → Shift_JIS → EUC-JP).
    pub source_encoding: Option<String>,
}

impl Default for JavaLspOptions {
    fn default() -> Self {
        Self {
            init_timeout: Duration::from_secs(60),
            source_roots: Vec::new(),
            source_encoding: None,
        }
    }
}

/// Counts of files sent to jdtls via `didOpen`, for the lift summary.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenStats {
    pub opened: usize,
    /// Opened, but some bytes could not be decoded and were replaced.
    pub lossy: usize,
    /// Could not be read at all (I/O error); not opened.
    pub failed: usize,
}

pub struct JavaLspResolver {
    client: Arc<Mutex<LspClient>>,
    workspace_root: PathBuf,
    /// FQN prefixes derived from source roots (e.g. `src.main.java.`), stripped
    /// when converting result paths to FQNs.
    source_root_prefixes: Vec<String>,
    forced_encoding: Option<&'static encoding_rs::Encoding>,
    opened_files: Mutex<HashSet<String>>,
    open_stats: Mutex<OpenStats>,
    cache: tokio::sync::RwLock<HashMap<String, Vec<ResolvedSymbol>>>,
}

impl JavaLspResolver {
    pub async fn new(workspace_root: &Path, options: JavaLspOptions) -> Result<Self> {
        // Canonicalize so file URIs are absolute (`file://./x` is rejected by
        // jdtls: the relative segment is parsed as a URI authority) and so
        // result paths from jdtls can be stripped back to workspace-relative.
        let workspace_root = workspace_root.canonicalize().map_err(|e| {
            anyhow::anyhow!(
                "Cannot canonicalize workspace root {}: {e}",
                workspace_root.display()
            )
        })?;
        let workspace_root = workspace_root.as_path();

        let forced_encoding = options
            .source_encoding
            .as_deref()
            .map(crate::encoding::resolve_encoding)
            .transpose()?;

        let jdtls_cmd = find_jdtls()?;
        tracing::info!(command = %jdtls_cmd, "Starting jdtls");

        let data_dir = std::env::temp_dir().join(format!(
            "specgraphen-jdt-{}",
            workspace_root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));

        // jdtls reads unopened files from disk with the JVM default charset, so
        // an explicit source encoding must reach the JVM too.
        let mut envs = Vec::new();
        if let Some(label) = &options.source_encoding {
            let existing = std::env::var("JAVA_TOOL_OPTIONS").unwrap_or_default();
            envs.push((
                "JAVA_TOOL_OPTIONS".to_string(),
                format!("{existing} -Dfile.encoding={label}")
                    .trim()
                    .to_string(),
            ));
        }

        let mut client = LspClient::spawn(
            &jdtls_cmd,
            &["-data", &data_dir.to_string_lossy()],
            workspace_root,
            &envs,
        )
        .await?;

        let source_roots = if options.source_roots.is_empty() {
            let detected = detect_source_roots(workspace_root, forced_encoding);
            tracing::info!(roots = ?detected, "Auto-detected Java source roots");
            detected
        } else {
            options.source_roots.clone()
        };

        // Without a build definition (pom.xml / build.gradle / .classpath), jdtls
        // treats the workspace as an "invisible project" and needs the source
        // roots and libraries passed explicitly to build a project model.
        let init_options = serde_json::json!({
            "settings": {
                "java": {
                    "project": {
                        "sourcePaths": source_roots,
                        "referencedLibraries": ["**/*.jar"]
                    }
                }
            }
        });

        client
            .initialize(workspace_root, Some(init_options))
            .await?;

        tracing::info!("Waiting for jdtls to index workspace...");
        match client.wait_for_ready(options.init_timeout).await {
            Ok(true) => tracing::info!("jdtls reported ServiceReady"),
            Ok(false) => tracing::warn!(
                timeout_secs = options.init_timeout.as_secs(),
                "jdtls did not report ServiceReady within the timeout; \
                 proceeding anyway (resolution may be incomplete)"
            ),
            Err(e) => return Err(e),
        }

        let source_root_prefixes = source_roots
            .iter()
            .map(|r| format!("{}.", r.trim_matches('/').replace(['/', '\\'], ".")))
            .collect();

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            workspace_root: workspace_root.to_path_buf(),
            source_root_prefixes,
            forced_encoding,
            opened_files: Mutex::new(HashSet::new()),
            open_stats: Mutex::new(OpenStats::default()),
            cache: tokio::sync::RwLock::new(HashMap::new()),
        })
    }

    /// didOpen accounting so callers can surface how many files were sent to
    /// jdtls, decoded lossily, or dropped.
    pub async fn open_stats(&self) -> OpenStats {
        *self.open_stats.lock().await
    }

    fn abs_path(&self, file: &str) -> PathBuf {
        if Path::new(file).is_absolute() {
            PathBuf::from(file)
        } else {
            self.workspace_root.join(file)
        }
    }

    /// jdtls only answers position-based requests for files opened as working
    /// copies, so lazily send `textDocument/didOpen` before the first request
    /// touching each file. Returns the file URI.
    async fn ensure_open(&self, client: &mut LspClient, file: &str) -> String {
        let abs = self.abs_path(file);
        let uri = path_to_uri(&abs);

        let mut opened = self.opened_files.lock().await;
        if opened.contains(&uri) {
            return uri;
        }

        let decoded = match tokio::fs::read(&abs).await {
            Ok(bytes) => crate::encoding::decode_source(&bytes, self.forced_encoding),
            Err(e) => {
                tracing::warn!(file = %abs.display(), "Failed to read file for didOpen: {e}");
                self.open_stats.lock().await.failed += 1;
                return uri;
            }
        };

        if decoded.lossy {
            tracing::debug!(
                file = %abs.display(),
                encoding = decoded.encoding,
                "didOpen text decoded lossily (some bytes replaced)"
            );
        }

        match client.did_open(&uri, "java", &decoded.text).await {
            Ok(()) => {
                opened.insert(uri.clone());
                let mut stats = self.open_stats.lock().await;
                stats.opened += 1;
                if decoded.lossy {
                    stats.lossy += 1;
                }
            }
            Err(e) => {
                tracing::warn!(file = %abs.display(), "didOpen failed: {e}");
                self.open_stats.lock().await.failed += 1;
            }
        }

        uri
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

        let mut client = self.client.lock().await;
        let file = self.ensure_open(&mut client, &ctx.file).await;

        let results = match client
            .definition(&file, ctx.line.saturating_sub(1), ctx.column)
            .await
        {
            Ok(locations) => locations
                .into_iter()
                .filter_map(|loc| {
                    let fqn =
                        uri_to_fqn(&loc.uri, &self.workspace_root, &self.source_root_prefixes);
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

        let mut client = self.client.lock().await;
        let file = self.ensure_open(&mut client, &ctx.file).await;

        let results = match client
            .definition(&file, ctx.line.saturating_sub(1), ctx.column)
            .await
        {
            Ok(locations) => locations
                .into_iter()
                .filter_map(|loc| {
                    let fqn =
                        uri_to_fqn(&loc.uri, &self.workspace_root, &self.source_root_prefixes);
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
        let mut client = self.client.lock().await;
        let file = self.ensure_open(&mut client, &ctx.file).await;

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

fn uri_to_fqn(uri: &str, workspace_root: &Path, source_root_prefixes: &[String]) -> Option<String> {
    let path = uri_to_path(uri);
    let rel = Path::new(&path)
        .strip_prefix(workspace_root)
        .ok()?
        .to_string_lossy()
        .to_string();

    // Convert file path to FQN: src/main/java/com/example/Foo.java → com.example.Foo
    let fqn = rel.trim_end_matches(".java").replace(['/', '\\'], ".");

    // Strip the source root prefix (detected/configured roots first, then common defaults)
    let fqn = source_root_prefixes
        .iter()
        .find_map(|p| fqn.strip_prefix(p.as_str()))
        .or_else(|| fqn.strip_prefix("src.main.java."))
        .or_else(|| fqn.strip_prefix("src."))
        .unwrap_or(&fqn)
        .to_string();

    Some(fqn)
}

/// Detect source roots by comparing each Java file's `package` declaration with
/// its directory: for `a/b/com/example/Foo.java` declaring `package com.example;`,
/// the source root is `a/b`. Returns roots relative to the workspace root.
fn detect_source_roots(
    workspace_root: &Path,
    forced_encoding: Option<&'static encoding_rs::Encoding>,
) -> Vec<String> {
    let mut roots = BTreeSet::new();
    let mut stack = vec![workspace_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        // One .java file per directory is enough to determine its source root
        let mut dir_done = false;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if !name.starts_with('.') && name != "target" && name != "node_modules" {
                    stack.push(path);
                }
            } else if !dir_done && name.ends_with(".java") {
                dir_done = true;
                if let Some(root) = source_root_of(workspace_root, &dir, &path, forced_encoding) {
                    roots.insert(root);
                }
            }
        }
    }

    roots.into_iter().collect()
}

fn source_root_of(
    workspace_root: &Path,
    dir: &Path,
    java_file: &Path,
    forced_encoding: Option<&'static encoding_rs::Encoding>,
) -> Option<String> {
    let package = read_package_declaration(java_file, forced_encoding)?;
    let package_path = package.replace('.', "/");

    let rel_dir = dir
        .strip_prefix(workspace_root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");

    let root = rel_dir
        .strip_suffix(&package_path)?
        .trim_end_matches('/')
        .to_string();

    if root.is_empty() {
        None // package root is the workspace root itself; nothing to configure
    } else {
        Some(root)
    }
}

fn read_package_declaration(
    java_file: &Path,
    forced_encoding: Option<&'static encoding_rs::Encoding>,
) -> Option<String> {
    let content = crate::encoding::read_source(java_file, forced_encoding)
        .ok()?
        .text;
    for line in content.lines().take(100) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("package ") {
            return Some(rest.trim_end_matches(';').trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/simple-project")
    }

    #[test]
    fn detects_standard_maven_source_root() {
        let roots = detect_source_roots(&fixture_root(), None);
        assert_eq!(roots, vec!["src/main/java".to_string()]);
    }

    #[test]
    fn source_root_of_uses_package_declaration() {
        let root = fixture_root();
        let dir = root.join("src/main/java/com/example/model");
        let file = dir.join("User.java");
        assert_eq!(
            source_root_of(&root, &dir, &file, None),
            Some("src/main/java".to_string())
        );
    }

    #[test]
    fn source_root_of_rejects_mismatched_package() {
        let root = fixture_root();
        // Directory does not end with the declared package path
        let dir = root.join("src/main/java");
        let file = root.join("src/main/java/com/example/model/User.java");
        assert_eq!(source_root_of(&root, &dir, &file, None), None);
    }

    #[test]
    fn reads_package_declaration_from_shift_jis_file() {
        // "package com.example;\n// <SJIS 日本語コメント>\nclass A {}\n"
        let mut bytes = b"package com.example;\n// ".to_vec();
        bytes.extend_from_slice(&[0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA]); // 日本語 in Shift_JIS
        bytes.extend_from_slice(b"\nclass A {}\n");

        let dir = std::env::temp_dir().join("specgraphen-sjis-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("A.java");
        std::fs::write(&file, &bytes).unwrap();

        assert_eq!(
            read_package_declaration(&file, None),
            Some("com.example".to_string())
        );
    }

    #[test]
    fn uri_to_fqn_strips_detected_source_root() {
        let prefixes = vec!["server.javasrc.".to_string()];
        let fqn = uri_to_fqn(
            "file:///work/server/javasrc/com/example/Foo.java",
            Path::new("/work"),
            &prefixes,
        );
        assert_eq!(fqn, Some("com.example.Foo".to_string()));
    }

    #[test]
    fn uri_to_fqn_falls_back_to_common_roots() {
        let fqn = uri_to_fqn(
            "file:///work/src/main/java/com/example/Foo.java",
            Path::new("/work"),
            &[],
        );
        assert_eq!(fqn, Some("com.example.Foo".to_string()));
    }

    #[test]
    fn uri_to_fqn_ignores_paths_outside_workspace() {
        let fqn = uri_to_fqn(
            "file:///elsewhere/com/example/Foo.java",
            Path::new("/work"),
            &[],
        );
        assert_eq!(fqn, None);
    }
}
