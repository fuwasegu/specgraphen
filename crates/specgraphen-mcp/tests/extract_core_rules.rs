//! Integration test: lift the fixture project, expose it via the MCP server,
//! and call the extract_core_rules tool end to end.

use std::collections::HashMap;
use std::path::Path;

use specgraphen_mcp::SpecGraphenServer;
use specgraphen_query::QueryEngine;

fn fixture_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/simple-project")
}

fn server() -> SpecGraphenServer {
    let root = fixture_root();
    let mut lifter = specgraphen_lift::JavaLifter::new().unwrap();
    let config = specgraphen_lift::LiftConfig {
        root_path: root.clone(),
        space_id: "mcp-test".to_string(),
        space_label: "mcp-test".to_string(),
        ..Default::default()
    };
    let result = lifter.lift(&config).unwrap();

    // Same keying as `serve`: witness-relative path → content
    let mut sources: HashMap<String, String> = HashMap::new();
    for entity in &result.space_data.entities {
        let file = &entity.witness.file;
        if !file.is_empty() && !sources.contains_key(file) {
            if let Ok(content) = std::fs::read_to_string(root.join(file)) {
                sources.insert(file.clone(), content);
            }
        }
    }

    let engine = QueryEngine::new(result.space_data).with_sources(sources);
    SpecGraphenServer::new(engine)
}

fn call_tool(server: &SpecGraphenServer, symbol: &str) -> String {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "extract_core_rules",
            "arguments": { "symbol": symbol }
        }
    })
    .to_string();
    let response = server.handle_request_line(&request);
    let v: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert!(
        v.get("error").is_none(),
        "tool returned error: {}",
        v["error"]
    );
    v["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn method_symbol_returns_compressed_table() {
    let server = server();
    let text = call_tool(&server, "UserService.createUser");
    assert!(
        text.contains("com.example.service.UserService.createUser"),
        "{text}"
    );
    assert!(text.contains("| outcome |"), "{text}");
    assert!(text.contains("compressed to"), "{text}");
}

#[test]
fn class_symbol_returns_all_branching_methods() {
    let server = server();
    let text = call_tool(&server, "com.example.service.UserService");
    assert!(text.contains("createUser"), "{text}");
    assert!(text.contains("getUser"), "{text}");
}

#[test]
fn tool_is_listed() {
    let server = server();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    })
    .to_string();
    let response = server.handle_request_line(&request);
    assert!(response.contains("extract_core_rules"), "{response}");
}
