//! MCP server (stdio JSON-RPC) exposing specgraphen code intelligence tools.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specgraphen_query::QueryEngine;

pub struct SpecGraphenServer {
    query_engine: Arc<QueryEngine>,
    store_path: Option<PathBuf>,
    space_id: Option<String>,
}

impl SpecGraphenServer {
    pub fn new(query_engine: QueryEngine) -> Self {
        Self {
            query_engine: Arc::new(query_engine),
            store_path: None,
            space_id: None,
        }
    }

    pub fn with_store(mut self, store_path: PathBuf, space_id: String) -> Self {
        self.store_path = Some(store_path);
        self.space_id = Some(space_id);
        self
    }

    pub async fn run_stdio(&self) -> anyhow::Result<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if v.get("id").is_none() {
                    continue;
                }
            }

            let response = self.handle_request(&line);
            let response_str = serde_json::to_string(&response)?;
            stdout.write_all(response_str.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }

        Ok(())
    }

    fn handle_request(&self, input: &str) -> JsonRpcResponse {
        let request: JsonRpcRequest = match serde_json::from_str(input) {
            Ok(r) => r,
            Err(e) => {
                return JsonRpcResponse::error(
                    serde_json::Value::Null,
                    -32700,
                    format!("Parse error: {e}"),
                );
            }
        };

        let result = match request.method.as_str() {
            "initialize" => self.handle_initialize(),
            "tools/list" => self.handle_tools_list(),
            "tools/call" => self.handle_tools_call(&request.params),
            "ping" => Ok(serde_json::json!({})),
            _ => Err((-32601, format!("Method not found: {}", request.method))),
        };

        match result {
            Ok(value) => JsonRpcResponse::success(request.id, value),
            Err((code, message)) => JsonRpcResponse::error(request.id, code, message),
        }
    }

    fn handle_initialize(&self) -> Result<serde_json::Value, (i32, String)> {
        Ok(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "specgraphen",
                "version": "0.1.0"
            }
        }))
    }

    fn handle_tools_list(&self) -> Result<serde_json::Value, (i32, String)> {
        let space = self.query_engine.space_data();
        let entity_count = space.cells.len();
        let relation_count = space.incidences.len();

        Ok(serde_json::json!({
            "tools": [
                {
                    "name": "overview",
                    "description": format!("Get a full overview of the analyzed Java codebase ({entity_count} entities, {relation_count} relations). Returns: entity/relation counts by type, package structure with class/method/field counts."),
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                },
                {
                    "name": "search",
                    "description": "Search for Java entities (classes, methods, fields, etc.) by name. Supports partial matching. Returns FQN, type, file location, and line number.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Search query (partial name match, e.g., 'Order', 'login', 'Customer')"
                            },
                            "entity_type": {
                                "type": "string",
                                "description": "Optional filter: class, interface, method, field, constructor, enum, package"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Max results (default 30)"
                            }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "package_dependencies",
                    "description": "Show cross-package dependency graph. Returns packages as nodes and inter-package calls/references as edges with counts. Use this to understand the overall architecture.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                },
                {
                    "name": "class_dependencies",
                    "description": "Show dependencies of a specific class: what it depends on and what depends on it. Returns a dependency graph centered on the given class.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "class": {
                                "type": "string",
                                "description": "Fully qualified or simple class name"
                            }
                        },
                        "required": ["class"]
                    }
                },
                {
                    "name": "explain",
                    "description": "Explain a Java code symbol's meaning with evidence-backed confidence. Returns: signature, intent, behavior, pre/post conditions, side effects, error behavior, source witnesses (file:line), callers, and callees.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Fully qualified name (e.g., com.example.UserService) or simple name"
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "callers",
                    "description": "List all verified callers of a Java code symbol with confidence and source witnesses.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Fully qualified or simple symbol name"
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "callees",
                    "description": "List all verified callees of a Java code symbol with confidence and source witnesses.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Fully qualified or simple symbol name"
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "column_usage",
                    "description": "Analyze how each column (field) of a data/table class is used across the codebase. Returns: column name, logical name (from JPA @Column annotations, doc comments, or DDL COMMENT), data type, and all read/write sites with file:line — including references inside SQL statements and .sql files. Use this to understand what each DB column is for and where it's accessed.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "table": {
                                "type": "string",
                                "description": "Table/data class name (e.g., 'User', 'Order', 'Product')"
                            }
                        },
                        "required": ["table"]
                    }
                },
                {
                    "name": "feature",
                    "description": "Analyze a business feature by keyword. Finds all related classes, entry points (methods called from outside), internal call flow, external dependencies, and data entities. Use this to understand a feature's scope and structure (e.g., 'Order', 'Login', 'Password', 'Mail').",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "keyword": {
                                "type": "string",
                                "description": "Feature keyword to search for (e.g., 'Order', 'Login', 'Customer', 'Mail')"
                            }
                        },
                        "required": ["keyword"]
                    }
                },
                {
                    "name": "impact",
                    "description": "Analyze the impact of changing a symbol. Shows what would be affected if the given class/method is modified — direct callers, transitive dependents, and affected files. Use this before making changes.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Symbol to analyze change impact for"
                            },
                            "max_depth": {
                                "type": "integer",
                                "description": "Max traversal depth (default 3)"
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "unknowns",
                    "description": "List ambiguous points, unresolved references, and low-confidence entities — the known unknowns. These are areas where AI should focus its reasoning. Optionally filter by scope (package or class name).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "scope": {
                                "type": "string",
                                "description": "Optional scope filter (package or class name)"
                            }
                        },
                        "required": []
                    }
                },
                {
                    "name": "enrich",
                    "description": "Get source code and structural context for a symbol, ready for semantic analysis. Returns the source code, callers, callees, containing class, and analysis instructions. Use this to understand a symbol deeply, then call `annotate` to save your analysis.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Symbol to get enrichment context for"
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "enrich_batch",
                    "description": "Get a batch of unannotated entities that need semantic analysis. Returns source code and context for each. Use this to systematically enrich the codebase knowledge — process each entity and call `annotate` for each.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "scope": {
                                "type": "string",
                                "description": "Optional scope filter (package or class name, e.g. 'Order', 'com.example.service')"
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Max entities to return (default 10)"
                            }
                        },
                        "required": []
                    }
                },
                {
                    "name": "annotate",
                    "description": "Save a semantic annotation for a symbol. Call this after analyzing source code (from `enrich`) to record intent, behavior, pre/post conditions, side effects, and error behavior. This enriches subsequent `explain` queries.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Fully qualified symbol name"
                            },
                            "intent": {
                                "type": "string",
                                "description": "One-line purpose description"
                            },
                            "behavior": {
                                "type": "string",
                                "description": "Step-by-step behavioral description"
                            },
                            "preconditions": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "List of preconditions"
                            },
                            "postconditions": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "List of postconditions"
                            },
                            "side_effects": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "List of side effects"
                            },
                            "error_behavior": {
                                "type": "string",
                                "description": "How errors are handled"
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "save",
                    "description": "Persist all annotations to disk. Call this after a batch of `annotate` calls to save the enriched space data.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                },
                {
                    "name": "dead_code",
                    "description": "Find unused methods and classes: entities with no callers or references in the analyzed sources. Each finding has a confidence level (high = private and unreferenced, medium = public but unreferenced, low = possibly framework-invoked) and a reason. Use this to identify deletion candidates in legacy code.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                },
                {
                    "name": "hotspots",
                    "description": "Rank methods by approximate cyclomatic complexity and size. Use this to triage where to start reading or refactoring a legacy codebase.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {
                                "type": "number",
                                "description": "Max results (default 20)"
                            }
                        },
                        "required": []
                    }
                },
                {
                    "name": "crud_matrix",
                    "description": "Build an entry-point × table CRUD matrix: which entry points create/read/update/delete which data classes, derived from SQL statements (Java strings, .sql files, MyBatis mapper XML) and repository naming conventions reached over the call graph. A classic legacy-analysis deliverable.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                },
                {
                    "name": "export_spec",
                    "description": "Export a Markdown specification document built from the lifted structure and all accumulated semantic annotations (intent, behavior, contracts). Use after enriching the space to produce a human-readable spec for the legacy codebase.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                }
            ]
        }))
    }

    fn handle_tools_call(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, (i32, String)> {
        let tool_name = params["name"]
            .as_str()
            .ok_or((-32602, "Missing tool name".to_string()))?;
        let arguments = &params["arguments"];

        let result_text = match tool_name {
            "overview" => {
                let result = self
                    .query_engine
                    .overview()
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "search" => {
                let query = arguments["query"]
                    .as_str()
                    .ok_or((-32602, "Missing query argument".to_string()))?;
                let entity_type = arguments["entity_type"].as_str();
                let limit = arguments["limit"].as_u64().unwrap_or(30) as usize;
                let result = self
                    .query_engine
                    .search(query, entity_type, limit)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "package_dependencies" => {
                let result = self
                    .query_engine
                    .package_dependencies()
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "class_dependencies" => {
                let class = arguments["class"]
                    .as_str()
                    .ok_or((-32602, "Missing class argument".to_string()))?;
                let result = self
                    .query_engine
                    .class_dependencies(class)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "explain" => {
                let symbol = arguments["symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing symbol argument".to_string()))?;
                let result = self
                    .query_engine
                    .explain(symbol)
                    .map_err(|e| (-32000, format!("Symbol not found: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "callers" => {
                let symbol = arguments["symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing symbol argument".to_string()))?;
                let result = self
                    .query_engine
                    .callers(symbol)
                    .map_err(|e| (-32000, format!("Symbol not found: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "callees" => {
                let symbol = arguments["symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing symbol argument".to_string()))?;
                let result = self
                    .query_engine
                    .callees(symbol)
                    .map_err(|e| (-32000, format!("Symbol not found: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "column_usage" => {
                let table = arguments["table"]
                    .as_str()
                    .ok_or((-32602, "Missing table argument".to_string()))?;
                let result = self
                    .query_engine
                    .column_usage(table)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "feature" => {
                let keyword = arguments["keyword"]
                    .as_str()
                    .ok_or((-32602, "Missing keyword argument".to_string()))?;
                let result = self
                    .query_engine
                    .feature(keyword)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "impact" => {
                let symbol = arguments["symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing symbol argument".to_string()))?;
                let max_depth = arguments["max_depth"].as_u64().unwrap_or(3) as usize;
                let result = self
                    .query_engine
                    .impact(symbol, max_depth)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "unknowns" => {
                let scope = arguments["scope"].as_str();
                let result = self
                    .query_engine
                    .unknowns(scope)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "enrich" => {
                let symbol = arguments["symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing symbol argument".to_string()))?;
                let result = self
                    .query_engine
                    .enrich(symbol)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "enrich_batch" => {
                let scope = arguments["scope"].as_str();
                let limit = arguments["limit"].as_u64().unwrap_or(10) as usize;
                let result = self
                    .query_engine
                    .enrich_batch(scope, limit)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "annotate" => {
                let symbol = arguments["symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing symbol argument".to_string()))?;
                let annotation = specgraphen_model::SemanticAnnotation {
                    intent: arguments["intent"].as_str().map(String::from),
                    behavior: arguments["behavior"].as_str().map(String::from),
                    preconditions: arguments["preconditions"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    postconditions: arguments["postconditions"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    invariants: Vec::new(),
                    side_effects: arguments["side_effects"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    error_behavior: arguments["error_behavior"].as_str().map(String::from),
                };
                self.query_engine
                    .annotate_by_fqn(symbol, annotation)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                format!("{{\"status\": \"ok\", \"symbol\": \"{symbol}\"}}")
            }
            "dead_code" => {
                let result = self
                    .query_engine
                    .dead_code()
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "hotspots" => {
                let limit = arguments["limit"].as_u64().unwrap_or(20) as usize;
                let result = self
                    .query_engine
                    .hotspots(limit)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "crud_matrix" => {
                let result = self
                    .query_engine
                    .crud_matrix()
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "export_spec" => self
                .query_engine
                .spec_markdown()
                .map_err(|e| (-32000, format!("Error: {e}")))?,
            "save" => {
                let snapshot = self
                    .query_engine
                    .save_snapshot()
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                if let (Some(store_path), Some(space_id)) = (&self.store_path, &self.space_id) {
                    let dir = store_path.join("spaces").join(space_id);
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| (-32000, format!("Failed to create dir: {e}")))?;
                    let path = dir.join("space.json");
                    let json = serde_json::to_string_pretty(&snapshot)
                        .map_err(|e| (-32000, format!("Serialization error: {e}")))?;
                    std::fs::write(&path, json)
                        .map_err(|e| (-32000, format!("Write error: {e}")))?;
                    format!(
                        "{{\"status\": \"saved\", \"path\": \"{}\", \"annotations\": {}}}",
                        path.display(),
                        snapshot.annotations.len()
                    )
                } else {
                    return Err((-32000, "Store path not configured".to_string()));
                }
            }
            _ => return Err((-32602, format!("Unknown tool: {tool_name}"))),
        };

        Ok(serde_json::json!({
            "content": [{"type": "text", "text": result_text}]
        }))
    }
}

#[derive(Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: serde_json::Value,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: serde_json::Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}
