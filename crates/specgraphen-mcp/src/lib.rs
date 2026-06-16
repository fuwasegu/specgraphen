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

    /// Handle one raw JSON-RPC request and return the serialized response.
    /// Useful for embedding and integration tests; `run_stdio` is a loop
    /// over this.
    pub fn handle_request_line(&self, input: &str) -> String {
        serde_json::to_string(&self.handle_request(input)).expect("response serializes")
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
                    "name": "enforces",
                    "description": "Check whether a required relation (e.g. 'java.calls') is reachable from an entry symbol within a depth bound, via HG bounded model checking. Returns satisfied/violated/unknown with visited cells and obstructions.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "entry_symbol": {
                                "type": "string",
                                "description": "Entry symbol to start the bounded check from"
                            },
                            "required_relation": {
                                "type": "string",
                                "description": "Relation type that must occur (e.g. 'java.calls')"
                            },
                            "max_depth": {
                                "type": "integer",
                                "description": "Max traversal depth (default 5)"
                            }
                        },
                        "required": ["entry_symbol", "required_relation"]
                    }
                },
                {
                    "name": "spec_loss",
                    "description": "Measure information loss of the human spec projection via HG's projection kernel — omitted entities, members folded into class sections, undeclared loss, and an overall review risk severity. Use to gauge how complete/honest the generated spec is.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                },
                {
                    "name": "domain_clusters",
                    "description": "Detect domain clusters via topological data analysis (persistent homology over the call graph) and flag clusters that span multiple packages — refactor/boundary candidates. Higher min_lifetime keeps only the most stable clusters.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "min_lifetime": {
                                "type": "integer",
                                "description": "Minimum persistence lifetime (stages) to keep a cluster (default 2)"
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
                },
                {
                    "name": "extract_core_rules",
                    "description": "Flatten a method's branching logic into a mathematically minimized decision table (Quine-McCluskey). Enumerates all execution paths through if/else (decomposing && and || with short-circuit semantics), then removes conditions that provably never influence any outcome — these 'dead variables' are typically leftover patch noise, and the surviving rules are the true specification. Pass a method FQN for one method or a class FQN for every branching method in the class. Requires the server to be started with --source-root.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Method or class FQN (partial suffix match supported, e.g. 'UserService.createUser' or 'UserService')"
                            },
                            "terminal_calls": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Method names that terminate the process like System.exit (e.g. a legacy error-exit helper); paths end at such calls instead of flowing to an unreachable return"
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "debug_trace",
                    "description": "Runtime-less interactive symbolic stepping of a method. Replays a list of branch choices and reports what executed, the current symbolic variable values and path conditions ('Context Rules'), the call stack, and either the next branch to choose or the outcome. The session is stateless: pass the growing `choices` array each call (undo = drop the last; time-travel = truncate). Start with choices=[] to reach the first branch, then append a branch index to explore a world line. An agent can DFS the choice space to enumerate every behavior. No runtime, no side effects. Requires the server to be started with --source-root.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Method FQN (partial suffix match supported, e.g. 'UserService.createUser')"
                            },
                            "choices": {
                                "type": "array",
                                "items": {"type": "integer"},
                                "description": "Branch indices chosen so far, in order. Each index selects one world-line from the branches reported by the previous call. Empty = run to the first branch."
                            },
                            "terminal_calls": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Method names that terminate the process like System.exit (legacy error-exit helpers)."
                            }
                        },
                        "required": ["symbol"]
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
            "enforces" => {
                let entry = arguments["entry_symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing entry_symbol argument".to_string()))?;
                let relation = arguments["required_relation"]
                    .as_str()
                    .ok_or((-32602, "Missing required_relation argument".to_string()))?;
                let max_depth = arguments["max_depth"].as_u64().unwrap_or(5) as usize;
                let result = self
                    .query_engine
                    .enforces(entry, relation, max_depth)
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "spec_loss" => {
                let result = self
                    .query_engine
                    .spec_loss_report()
                    .map_err(|e| (-32000, format!("Error: {e}")))?;
                serde_json::to_string_pretty(&result)
                    .map_err(|e| (-32000, format!("Serialization error: {e}")))?
            }
            "domain_clusters" => {
                let min_lifetime = arguments["min_lifetime"].as_u64().unwrap_or(2) as usize;
                let result = self
                    .query_engine
                    .domain_clusters(min_lifetime)
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
            "extract_core_rules" => {
                let symbol = arguments["symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing symbol argument".to_string()))?;
                let terminal_calls: Vec<String> = arguments["terminal_calls"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.extract_core_rules(symbol, terminal_calls)?
            }
            "debug_trace" => {
                let symbol = arguments["symbol"]
                    .as_str()
                    .ok_or((-32602, "Missing symbol argument".to_string()))?;
                let choices: Vec<usize> = arguments["choices"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_u64().map(|n| n as usize))
                            .collect()
                    })
                    .unwrap_or_default();
                let terminal_calls: Vec<String> = arguments["terminal_calls"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.debug_trace(symbol, &choices, &terminal_calls)?
            }
            _ => return Err((-32602, format!("Unknown tool: {tool_name}"))),
        };

        Ok(serde_json::json!({
            "content": [{"type": "text", "text": result_text}]
        }))
    }

    fn debug_trace(
        &self,
        symbol: &str,
        choices: &[usize],
        terminal_calls: &[String],
    ) -> Result<String, (i32, String)> {
        let (fqn, file, line) = self
            .query_engine
            .witness_of(symbol)
            .ok_or((-32000, format!("Symbol not found: {symbol}")))?;
        let source = self.query_engine.file_source(&file).ok_or((
            -32000,
            format!(
                "Source for {file} is not loaded. Start the server with \
                 --source-root pointing at the Java source tree."
            ),
        ))?;

        let result = specgraphen_lift::trace(source, line, choices, terminal_calls)
            .map_err(|e| (-32000, format!("Trace failed: {e}")))?;

        let mut out = String::new();
        out.push_str(&format!("# Debug trace: {fqn} ({file}:{line})\n\n"));
        if !choices.is_empty() {
            out.push_str(&format!("Choices replayed: {choices:?}\n\n"));
        }

        out.push_str("## Executed\n");
        if result.executed.is_empty() {
            out.push_str("(nothing — already at a branch or the start)\n");
        } else {
            for step in &result.executed {
                out.push_str(&format!("- L{}  {}\n", step.line, step.text));
            }
        }

        out.push_str("\n## State\n");
        out.push_str(&format!(
            "- Call stack: {}\n",
            result.call_stack.join(" › ")
        ));
        if result.variables.is_empty() {
            out.push_str("- Variables: (none)\n");
        } else {
            let vars: Vec<String> = result
                .variables
                .iter()
                .map(|(k, v)| format!("{k} = {v}"))
                .collect();
            out.push_str(&format!("- Variables: {}\n", vars.join(", ")));
        }
        if result.context_rules.is_empty() {
            out.push_str("- Context Rules: (none — no branch taken)\n");
        } else {
            out.push_str(&format!(
                "- Context Rules: {}\n",
                result.context_rules.join(" ∧ ")
            ));
        }

        if result.incomplete {
            out.push_str(
                "\n> ⚠ incomplete: this path stepped over an unmodeled construct \
                 (switch/try) that can itself return/throw — the outcome below may \
                 not be the method's real behavior on this path.\n",
            );
        }

        out.push_str("\n## Status\n");
        match result.status {
            specgraphen_lift::TraceStatus::AwaitingChoice { branches } => {
                out.push_str("Paused at a branch — choose a world line by appending its index to `choices`:\n");
                for (i, b) in branches.iter().enumerate() {
                    out.push_str(&format!("- {i}: {b}\n"));
                }
            }
            specgraphen_lift::TraceStatus::Terminated { outcome } => {
                out.push_str(&format!("Terminated → {outcome}\n"));
            }
            specgraphen_lift::TraceStatus::FallThrough => {
                out.push_str("Fell through to the end of the method (no explicit return).\n");
            }
            specgraphen_lift::TraceStatus::StepCap => {
                out.push_str("Hit the step cap before reaching a branch or end (method too large to trace in one call).\n");
            }
        }
        Ok(out)
    }

    fn extract_core_rules(
        &self,
        symbol: &str,
        terminal_calls: Vec<String>,
    ) -> Result<String, (i32, String)> {
        let (fqn, file, _) = self
            .query_engine
            .witness_of(symbol)
            .ok_or((-32000, format!("Symbol not found: {symbol}")))?;

        let source = self.query_engine.file_source(&file).ok_or((
            -32000,
            format!(
                "Source for {file} is not loaded. Start the server with \
                 --source-root pointing at the Java source tree."
            ),
        ))?;

        let mut extractor = specgraphen_lift::DecisionExtractor::new()
            .map_err(|e| (-32000, format!("Extractor init failed: {e}")))?
            .with_terminal_calls(terminal_calls);
        let extraction = extractor
            .extract(source)
            .map_err(|e| (-32000, format!("Parse failed for {file}: {e}")))?;

        // Method FQN → that method; class FQN → every branching method in it
        let selected: Vec<_> = extraction
            .methods
            .iter()
            .filter(|d| d.fqn() == fqn || d.class_fqn == fqn)
            .collect();

        let mut out = String::new();
        for decision in &selected {
            out.push_str(&format!(
                "## {} ({file}:{})\n\n",
                decision.fqn(),
                decision.start_line
            ));
            if decision.incomplete {
                out.push_str(
                    "> ⚠ incomplete: the body contains unmodeled exits \
                     (loop/switch/try); outcomes inside those are not in the table\n\n",
                );
            }
            match specgraphen_logic::compress(&decision.table) {
                Ok(compressed) => {
                    out.push_str(&format!(
                        "{} observed paths compressed to {} rules.\n\n",
                        decision.table.rows().len(),
                        compressed.rules.len()
                    ));
                    out.push_str(&compressed.to_markdown());
                    out.push('\n');
                }
                Err(e) => {
                    // A conflict is a genuine finding about the source logic
                    out.push_str(&format!("> ✗ not compressible: {e}\n\n"));
                }
            }
        }

        let skipped: Vec<_> = extraction
            .skipped
            .iter()
            .filter(|(m, _)| *m == fqn || m.starts_with(&format!("{fqn}.")))
            .collect();
        for (method, reason) in &skipped {
            out.push_str(&format!("- skipped {method}: {reason}\n"));
        }

        if selected.is_empty() && skipped.is_empty() {
            out = format!(
                "{fqn} has no extractable branching logic \
                 (no if/else, or only unmodeled constructs)."
            );
        }
        Ok(out)
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
