use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "specgraphen",
    version,
    about = "AI-operated code meaning extraction substrate"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Lift {
        #[arg(long, default_value = ".")]
        root: String,
        #[arg(long)]
        space_id: String,
        #[arg(long, default_value = ".specgraphen")]
        store: String,
        /// Enable LLM corroboration (claude or openai)
        #[arg(long)]
        llm_provider: Option<String>,
        /// API key for LLM provider (or set ANTHROPIC_API_KEY / OPENAI_API_KEY)
        #[arg(long)]
        llm_api_key: Option<String>,
        /// LLM model name
        #[arg(long)]
        llm_model: Option<String>,
        /// LLM API base URL (optional override)
        #[arg(long)]
        llm_base_url: Option<String>,
        /// Enable LSP type resolution (java)
        #[arg(long)]
        lsp: Option<String>,
        /// Seconds to wait for the LSP server to finish indexing (default: 60)
        #[arg(long, default_value_t = 60)]
        lsp_init_timeout: u64,
        /// Java source root relative to --root (repeatable; auto-detected if omitted)
        #[arg(long)]
        source_root: Vec<String>,
        /// Source file encoding for LSP (e.g. shift_jis, windows-31j; auto-detected if omitted)
        #[arg(long)]
        source_encoding: Option<String>,
    },
    Query {
        #[command(subcommand)]
        query: QueryCommands,
        #[arg(long, default_value = ".specgraphen")]
        store: String,
        #[arg(long)]
        space_id: String,
    },
    Serve {
        #[arg(long, default_value = "stdio")]
        transport: String,
        #[arg(long, default_value = ".specgraphen")]
        store: String,
        #[arg(long)]
        space_id: String,
        /// Root directory of source code (for enrich tool)
        #[arg(long)]
        source_root: Option<String>,
    },
    /// Extract compressed decision tables (core rules) from Java methods
    Rules {
        /// Root directory of Java source code
        #[arg(long)]
        root: String,
        /// Only report methods whose FQN contains this substring
        #[arg(long)]
        method: Option<String>,
        /// Source file encoding (e.g. shift_jis; auto-detected if omitted)
        #[arg(long)]
        source_encoding: Option<String>,
        /// Method name that terminates the process like System.exit
        /// (repeatable, e.g. --terminal-call abortOnError)
        #[arg(long)]
        terminal_call: Vec<String>,
    },
    /// Export a Markdown specification from the lifted space and its annotations
    Export {
        #[arg(long, default_value = ".specgraphen")]
        store: String,
        #[arg(long)]
        space_id: String,
        /// Output file (stdout if omitted)
        #[arg(long)]
        out: Option<String>,
    },
}

#[derive(Subcommand)]
enum QueryCommands {
    Explain {
        symbol: String,
    },
    Callers {
        symbol: String,
    },
    Callees {
        symbol: String,
    },
    /// Bounded model-check: is `relation` reachable from `entry`?
    Enforces {
        entry: String,
        relation: String,
        #[arg(long, default_value_t = 5)]
        max_depth: usize,
    },
    /// Measure information loss of the human spec projection.
    SpecLoss,
    /// Detect domain clusters (TDA) and package-boundary drift.
    DomainClusters {
        #[arg(long, default_value_t = 2)]
        min_lifetime: usize,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Lift {
            root,
            space_id,
            store,
            llm_provider,
            llm_api_key,
            llm_model,
            llm_base_url,
            lsp,
            lsp_init_timeout,
            source_root,
            source_encoding,
        } => {
            let mut lifter = specgraphen_lift::JavaLifter::new()?;

            // Build LSP resolver cache if requested
            let resolved_cache = if let Some(ref lsp_lang) = lsp {
                match lsp_lang.as_str() {
                    "java" => {
                        let lsp_options = specgraphen_resolver::java::JavaLspOptions {
                            init_timeout: std::time::Duration::from_secs(lsp_init_timeout),
                            source_roots: source_root,
                            source_encoding,
                        };
                        build_lsp_cache_java(&mut lifter, &root, &space_id, lsp_options).await
                    }
                    other => {
                        tracing::warn!("Unknown LSP language: {other}. Supported: java");
                        std::collections::HashMap::new()
                    }
                }
            } else {
                std::collections::HashMap::new()
            };

            let config = specgraphen_lift::LiftConfig {
                root_path: PathBuf::from(&root),
                space_id: space_id.clone(),
                space_label: space_id.clone(),
                resolved_cache,
                ..Default::default()
            };

            let mut result = lifter.lift(&config)?;

            let entity_count = result.space_data.cells.len();
            let relation_count = result.space_data.incidences.len();
            let warning_count = result
                .diagnostics
                .iter()
                .filter(|d| matches!(d.severity, specgraphen_lift::DiagnosticSeverity::Warning))
                .count();
            let error_count = result
                .diagnostics
                .iter()
                .filter(|d| matches!(d.severity, specgraphen_lift::DiagnosticSeverity::Error))
                .count();

            // Run LLM corroboration if configured
            let llm_provider_instance = build_llm_provider(
                llm_provider.as_deref(),
                llm_api_key.as_deref(),
                llm_model.as_deref(),
                llm_base_url.as_deref(),
            );

            let corroboration_config = specgraphen_corroboration::CorroborationConfig {
                enable_llm_pass: llm_provider_instance.is_some(),
                ..Default::default()
            };

            let mut engine = specgraphen_corroboration::CorroborationEngine::new(
                llm_provider_instance,
                corroboration_config,
            );
            engine.load_sources(&PathBuf::from(&root))?;

            let stats = engine.corroborate(&mut result.space_data).await?;
            tracing::info!(
                corroborated = stats.corroborated,
                parse_failures = stats.parse_failures,
                llm_errors = stats.llm_errors,
                "Corroboration complete"
            );

            // Run invariant checks
            let checker = specgraphen_invariant::InvariantChecker::default_checks();
            let violations = checker.check_all(&result.space_data);

            let file_store = specgraphen_store::JsonFileStore::new(&store);
            use specgraphen_store::SpaceStore;
            file_store.save(&result.space_data).await?;

            println!("Lift complete:");
            println!("  Entities:    {entity_count}");
            println!("  Relations:   {relation_count}");
            println!("  Warnings:    {warning_count}");
            println!("  Errors:      {error_count}");
            if stats.corroborated > 0 {
                println!("  LLM corroborated: {}", stats.corroborated);
                println!("  LLM parse fails:  {}", stats.parse_failures);
                println!("  LLM errors:       {}", stats.llm_errors);
            }
            println!("  Invariant violations: {}", violations.len());
            println!("  Stored at:   {store}/spaces/{space_id}/space.json");
        }
        Commands::Query {
            query,
            store,
            space_id,
        } => {
            let file_store = specgraphen_store::JsonFileStore::new(&store);
            use specgraphen_store::SpaceStore;
            let space_data = file_store.load(&space_id).await?;
            let engine = specgraphen_query::QueryEngine::new(space_data);

            match query {
                QueryCommands::Explain { symbol } => {
                    let result = engine.explain(&symbol)?;
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                QueryCommands::Callers { symbol } => {
                    let result = engine.callers(&symbol)?;
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                QueryCommands::Callees { symbol } => {
                    let result = engine.callees(&symbol)?;
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                QueryCommands::Enforces {
                    entry,
                    relation,
                    max_depth,
                } => {
                    let result = engine.enforces(&entry, &relation, max_depth)?;
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                QueryCommands::SpecLoss => {
                    let result = engine.spec_loss_report()?;
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                QueryCommands::DomainClusters { min_lifetime } => {
                    let result = engine.domain_clusters(min_lifetime)?;
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
            }
        }
        Commands::Serve {
            transport,
            store,
            space_id,
            source_root,
        } => {
            let file_store = specgraphen_store::JsonFileStore::new(&store);
            use specgraphen_store::SpaceStore;
            let space_data = file_store.load(&space_id).await?;

            // Load source files for enrich tool
            let mut source_files = std::collections::HashMap::new();
            let source_root_path = source_root.as_deref().map(PathBuf::from);
            for entity in &space_data.entities {
                if !entity.witness.file.is_empty()
                    && !source_files.contains_key(&entity.witness.file)
                {
                    let mut possible_paths = vec![PathBuf::from(&entity.witness.file)];
                    if let Some(ref root) = source_root_path {
                        possible_paths.insert(0, root.join(&entity.witness.file));
                    }
                    for path in &possible_paths {
                        if path.exists() {
                            if let Ok(bytes) = std::fs::read(path) {
                                let content = if let Ok(s) = std::str::from_utf8(&bytes) {
                                    s.to_string()
                                } else {
                                    let (decoded, _, _) = encoding_rs::SHIFT_JIS.decode(&bytes);
                                    decoded.into_owned()
                                };
                                source_files.insert(entity.witness.file.clone(), content);
                            }
                            break;
                        }
                    }
                }
            }
            // Load SQL and MyBatis mapper XML sources so column_usage and
            // crud_matrix can see DDL, queries, and mapper statements
            if let Some(ref root) = source_root_path {
                let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
                for ext in ["sql", "xml"] {
                    let pattern = canonical
                        .join(format!("**/*.{ext}"))
                        .to_string_lossy()
                        .to_string();
                    let Ok(paths) = glob::glob(&pattern) else {
                        continue;
                    };
                    for path in paths.flatten() {
                        if let Ok(bytes) = std::fs::read(&path) {
                            let content = if let Ok(s) = std::str::from_utf8(&bytes) {
                                s.to_string()
                            } else {
                                let (decoded, _, _) = encoding_rs::SHIFT_JIS.decode(&bytes);
                                decoded.into_owned()
                            };
                            // Only mapper XMLs are useful; skip other XML files
                            if ext == "xml" && !content.contains("<mapper") {
                                continue;
                            }
                            source_files.insert(path.to_string_lossy().to_string(), content);
                        }
                    }
                }
            }

            tracing::info!(
                source_files = source_files.len(),
                "Source files loaded for enrich"
            );

            let engine = specgraphen_query::QueryEngine::new(space_data).with_sources(source_files);
            let server = specgraphen_mcp::SpecGraphenServer::new(engine)
                .with_store(PathBuf::from(&store), space_id.clone());

            tracing::info!(%transport, %space_id, "Starting MCP server");
            server.run_stdio().await?;
        }
        Commands::Rules {
            root,
            method,
            source_encoding,
            terminal_call,
        } => {
            let forced = source_encoding
                .as_deref()
                .map(specgraphen_resolver::encoding::resolve_encoding)
                .transpose()?;

            let pattern = format!("{}/**/*.java", root.trim_end_matches('/'));
            let mut extractor =
                specgraphen_lift::DecisionExtractor::new()?.with_terminal_calls(terminal_call);
            let mut reported = 0usize;
            let mut all_skipped: Vec<(String, String)> = Vec::new();

            for entry in glob::glob(&pattern)?.flatten() {
                let source = match specgraphen_resolver::encoding::read_source(&entry, forced) {
                    Ok(decoded) => decoded.text,
                    Err(e) => {
                        tracing::warn!(file = %entry.display(), "skipping unreadable file: {e}");
                        continue;
                    }
                };
                let extraction = match extractor.extract(&source) {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::warn!(file = %entry.display(), "parse failed: {e}");
                        continue;
                    }
                };
                all_skipped.extend(extraction.skipped);

                for decision in extraction.methods {
                    let fqn = decision.fqn();
                    if let Some(filter) = &method {
                        if !fqn.contains(filter.as_str()) {
                            continue;
                        }
                    }
                    match specgraphen_logic::compress(&decision.table) {
                        Ok(compressed) => {
                            reported += 1;
                            println!("## {fqn} ({}:{})", entry.display(), decision.start_line);
                            if decision.incomplete {
                                println!(
                                    "\n> ⚠ incomplete: body contains unmodeled exits \
                                     (loop/switch/try)"
                                );
                            }
                            println!("\n{}", compressed.to_markdown());
                        }
                        Err(e) => {
                            // A conflict here is itself a finding about the code
                            println!("## {fqn} ({}:{})", entry.display(), decision.start_line);
                            println!("\n> ✗ not compressible: {e}\n");
                        }
                    }
                }
            }

            // JSP / tag files: JSTL conditional clusters (screen display logic)
            let tag_extractor = specgraphen_lift::TagExtractor::new();
            for ext in ["jsp", "tag"] {
                let pattern = format!("{}/**/*.{ext}", root.trim_end_matches('/'));
                for entry in glob::glob(&pattern)?.flatten() {
                    let path_str = entry.display().to_string();
                    if let Some(filter) = &method {
                        if !path_str.contains(filter.as_str()) {
                            continue;
                        }
                    }
                    let source = match specgraphen_resolver::encoding::read_source(&entry, forced) {
                        Ok(decoded) => decoded.text,
                        Err(e) => {
                            tracing::warn!(file = %path_str, "skipping unreadable file: {e}");
                            continue;
                        }
                    };
                    let extraction = tag_extractor.extract(&source);
                    all_skipped.extend(
                        extraction
                            .skipped
                            .into_iter()
                            .map(|(loc, reason)| (format!("{path_str} {loc}"), reason)),
                    );

                    for cluster in extraction.clusters {
                        match specgraphen_logic::compress(&cluster.table) {
                            Ok(compressed) => {
                                reported += 1;
                                println!("## {path_str}:{}", cluster.start_line);
                                if cluster.incomplete {
                                    println!(
                                        "\n> ⚠ incomplete: the file also contains scriptlet \
                                         conditionals (<% if %>) not in this table"
                                    );
                                }
                                println!("\n{}", compressed.to_markdown());
                            }
                            Err(e) => {
                                println!("## {path_str}:{}", cluster.start_line);
                                println!("\n> ✗ not compressible: {e}\n");
                            }
                        }
                    }
                }
            }

            if !all_skipped.is_empty() {
                println!("---");
                println!("Skipped methods:");
                for (fqn, reason) in &all_skipped {
                    println!("- {fqn}: {reason}");
                }
            }
            if reported == 0 {
                println!("(no branching methods matched)");
            }
        }
        Commands::Export {
            store,
            space_id,
            out,
        } => {
            let file_store = specgraphen_store::JsonFileStore::new(&store);
            use specgraphen_store::SpaceStore;
            let space_data = file_store.load(&space_id).await?;
            let markdown = specgraphen_query::export::spec_markdown(&space_data)?;
            match out {
                Some(path) => {
                    std::fs::write(&path, &markdown)?;
                    println!("Specification written to {path}");
                }
                None => print!("{markdown}"),
            }
        }
    }

    Ok(())
}

fn build_llm_provider(
    provider: Option<&str>,
    api_key: Option<&str>,
    model: Option<&str>,
    base_url: Option<&str>,
) -> Option<Arc<dyn specgraphen_llm::LlmProvider>> {
    let provider_type = provider?;

    let api_key = api_key.map(String::from).or_else(|| match provider_type {
        "claude" => std::env::var("ANTHROPIC_API_KEY").ok(),
        "openai" => std::env::var("OPENAI_API_KEY").ok(),
        _ => None,
    })?;

    let config = specgraphen_llm::config::LlmConfig {
        provider: match provider_type {
            "claude" => specgraphen_llm::config::ProviderType::Claude,
            "openai" => specgraphen_llm::config::ProviderType::OpenAi,
            _ => {
                tracing::warn!("Unknown LLM provider: {provider_type}");
                return None;
            }
        },
        api_key,
        model: model
            .map(String::from)
            .unwrap_or_else(|| match provider_type {
                "claude" => "claude-sonnet-4-20250514".to_string(),
                "openai" => "gpt-4o".to_string(),
                _ => "unknown".to_string(),
            }),
        base_url: base_url.map(String::from),
        ..Default::default()
    };

    match config.create_provider() {
        Ok(p) => {
            tracing::info!(
                provider = provider_type,
                model = config.model,
                "LLM provider configured"
            );
            Some(Arc::from(p))
        }
        Err(e) => {
            tracing::warn!("Failed to create LLM provider: {e}");
            None
        }
    }
}

async fn build_lsp_cache_java(
    lifter: &mut specgraphen_lift::JavaLifter,
    root: &str,
    space_id: &str,
    lsp_options: specgraphen_resolver::java::JavaLspOptions,
) -> std::collections::HashMap<String, String> {
    use specgraphen_resolver::TypeResolver;
    tracing::info!("Starting LSP resolver (jdtls)...");

    let root_path = PathBuf::from(root);

    // Step 1: Start jdtls
    let resolver =
        match specgraphen_resolver::java::JavaLspResolver::new(&root_path, lsp_options).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Failed to start jdtls: {e}. Falling back to heuristic resolution.");
                return std::collections::HashMap::new();
            }
        };
    tracing::info!("jdtls ready.");

    // Step 2: Run tree-sitter pass to collect unresolved symbols
    let pre_config = specgraphen_lift::LiftConfig {
        root_path: root_path.clone(),
        space_id: space_id.to_string(),
        space_label: space_id.to_string(),
        ..Default::default()
    };

    let unresolved = match lifter.collect_unresolved(&pre_config) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("Failed to collect unresolved symbols: {e}");
            return std::collections::HashMap::new();
        }
    };

    tracing::info!(
        unresolved = unresolved.len(),
        "Collected unresolved symbols for LSP resolution"
    );

    if unresolved.is_empty() {
        return std::collections::HashMap::new();
    }

    // Step 3: Ask jdtls to resolve each unresolved symbol
    let mut cache = std::collections::HashMap::new();
    let mut resolved_count = 0u32;
    let total = unresolved.len();

    // Deduplicate by (file, line, column) to avoid redundant LSP calls
    let mut seen = std::collections::HashSet::new();
    let mut unique_unresolved = Vec::new();
    for u in &unresolved {
        let key = format!("{}:{}:{}", u.file, u.line, u.column);
        if seen.insert(key) {
            unique_unresolved.push(u.clone());
        }
    }

    tracing::info!(
        unique = unique_unresolved.len(),
        total,
        "Deduped unresolved symbols"
    );

    for (i, u) in unique_unresolved.iter().enumerate() {
        let ctx = specgraphen_resolver::ResolveContext {
            // May be workspace-relative or absolute; the resolver normalizes it
            file: u.file.clone(),
            line: u.line,
            column: u.column,
            package: None,
            class_fqn: None,
        };

        let results = resolver
            .resolve_method_call(&u.target_text, None, &ctx)
            .await;
        if let Some(first) = results.first() {
            cache.insert(u.target_text.clone(), first.fqn.clone());
            resolved_count += 1;
        }

        if (i + 1) % 500 == 0 {
            tracing::info!(
                progress = i + 1,
                total = unique_unresolved.len(),
                resolved = resolved_count,
                "LSP resolution progress"
            );
        }
    }

    tracing::info!(
        resolved = resolved_count,
        total = unique_unresolved.len(),
        "LSP resolution complete"
    );

    let stats = resolver.open_stats().await;
    if stats.failed > 0 || stats.lossy > 0 {
        tracing::warn!(
            opened = stats.opened,
            lossy = stats.lossy,
            failed = stats.failed,
            "Some source files could not be opened cleanly for LSP \
             (consider --source-encoding, e.g. --source-encoding shift_jis)"
        );
    } else {
        tracing::info!(opened = stats.opened, "didOpen file stats");
    }

    cache
}
