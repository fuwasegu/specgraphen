use std::path::PathBuf;

use specgraphen_lift::{JavaLifter, LiftConfig};
use specgraphen_query::QueryEngine;

fn build_engine() -> QueryEngine {
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/simple-project");

    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path,
        space_id: "test".to_string(),
        space_label: "Test".to_string(),
        ..Default::default()
    };
    let result = lifter.lift(&config).expect("Lift failed");
    QueryEngine::new(result.space_data)
}

fn build_engine_with_sources() -> QueryEngine {
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/simple-project");

    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path.clone(),
        space_id: "test".to_string(),
        space_label: "Test".to_string(),
        ..Default::default()
    };
    let result = lifter.lift(&config).expect("Lift failed");

    let mut source_files = std::collections::HashMap::new();
    for entity in &result.space_data.entities {
        let file = &entity.witness.file;
        if file.is_empty() || source_files.contains_key(file) {
            continue;
        }
        let content = std::fs::read_to_string(file)
            .or_else(|_| std::fs::read_to_string(fixture_path.join(file)))
            .expect("Failed to read source file referenced by witness");
        source_files.insert(file.clone(), content);
    }

    // SQL sources, as the CLI `serve` command loads them
    for sql_file in ["db/schema.sql", "db/queries.sql"] {
        let content =
            std::fs::read_to_string(fixture_path.join(sql_file)).expect("Failed to read SQL file");
        source_files.insert(sql_file.to_string(), content);
    }

    QueryEngine::new(result.space_data).with_sources(source_files)
}

#[test]
fn test_dead_code_finds_unreferenced_methods_and_classes() {
    let engine = build_engine_with_sources();
    let result = engine.dead_code().expect("dead_code failed");

    assert!(
        result
            .unused_methods
            .iter()
            .any(|m| m.fqn.ends_with("NotificationService.sendWelcomeEmail")),
        "sendWelcomeEmail has no callers and should be reported"
    );
    assert!(
        !result
            .unused_methods
            .iter()
            .any(|m| m.fqn.ends_with("UserService.getUser")),
        "getUser is called by deleteUser and must not be reported"
    );
    assert!(
        result
            .unused_classes
            .iter()
            .any(|c| c.fqn.ends_with("NotificationService")),
        "NotificationService is never referenced and should be reported"
    );
    assert!(
        !result
            .unused_classes
            .iter()
            .any(|c| c.fqn.ends_with(".User")),
        "User is constructed by createUser and must not be reported"
    );
}

#[test]
fn test_hotspots_ranks_branchy_methods_first() {
    let engine = build_engine_with_sources();
    let result = engine.hotspots(5).expect("hotspots failed");

    assert!(!result.hotspots.is_empty());
    assert!(
        result.hotspots[0].fqn.ends_with("UserService.createUser"),
        "createUser has the most decision points, got {}",
        result.hotspots[0].fqn
    );
    assert!(result.hotspots[0].complexity >= 4);
    assert!(result.hotspots[0].loc > 0);
}

#[test]
fn test_crud_matrix_from_repository_conventions() {
    let engine = build_engine_with_sources();
    let result = engine.crud_matrix().expect("crud_matrix failed");

    let user = result
        .tables
        .iter()
        .find(|t| t.table_class == "com.example.model.User")
        .expect("User should be detected as a data class");
    assert_eq!(user.table_name, "user");

    let create = user
        .entries
        .iter()
        .find(|e| e.entry_point.ends_with("createUser"))
        .expect("createUser entry point should be present");
    assert!(
        create.operations.contains('C') && create.operations.contains('U'),
        "repository.save() implies C and U, got {}",
        create.operations
    );

    let delete = user
        .entries
        .iter()
        .find(|e| e.entry_point.ends_with("deleteUser"))
        .expect("deleteUser entry point should be present");
    assert!(
        delete.operations.contains('D'),
        "repository.deleteById() implies D, got {}",
        delete.operations
    );
    assert!(
        delete.operations.contains('R'),
        "deleteUser reaches getUser → findById, implying R, got {}",
        delete.operations
    );
}

#[test]
fn test_spec_markdown_includes_annotations() {
    let engine = build_engine_with_sources();
    engine
        .annotate_by_fqn(
            "com.example.service.UserService.createUser",
            specgraphen_model::SemanticAnnotation {
                intent: Some("Create a user after validating name and email".to_string()),
                ..Default::default()
            },
        )
        .expect("annotate failed");

    let markdown = engine.spec_markdown().expect("export failed");
    assert!(markdown.contains("# Specification:"));
    assert!(markdown.contains("### `UserService`"));
    assert!(markdown.contains("Create a user after validating name and email"));
    assert!(markdown.contains("_(not yet annotated)_"));
}

#[test]
fn test_column_usage_sql_sources() {
    let engine = build_engine_with_sources();
    let result = engine.column_usage("User").expect("column_usage failed");

    let email = result
        .columns
        .iter()
        .find(|c| c.field_name == "email")
        .expect("email column should be present");
    assert_eq!(
        email.logical_name, "Mail address",
        "logical name should fall back to the DDL COMMENT"
    );
    assert!(
        email
            .writers
            .iter()
            .any(|w| w.file.ends_with("queries.sql") && w.access_type == "write"),
        "UPDATE ... SET email should be detected as a write"
    );

    let id = result
        .columns
        .iter()
        .find(|c| c.field_name == "id")
        .expect("id column should be present");
    assert!(
        id.readers.iter().any(|r| r.file.ends_with("queries.sql")),
        "WHERE id = ? should be detected as a read"
    );
}

#[test]
fn test_column_usage_plain_java_class() {
    let engine = build_engine_with_sources();
    let result = engine.column_usage("User").expect("column_usage failed");

    assert_eq!(result.table_class, "com.example.model.User");

    let email = result
        .columns
        .iter()
        .find(|c| c.field_name == "email")
        .expect("email column should be present");
    assert_eq!(email.data_type, "String");
    assert_eq!(email.column_name, "email");
    assert!(
        !email.readers.is_empty(),
        "getEmail() callers should be detected as readers"
    );

    let id = result
        .columns
        .iter()
        .find(|c| c.field_name == "id")
        .expect("id column should be present");
    assert_eq!(id.data_type, "Long");
}

#[test]
fn test_explain_method() {
    let engine = build_engine();
    let result = engine
        .explain("com.example.service.UserService.createUser")
        .expect("explain failed");

    assert_eq!(result.symbol, "com.example.service.UserService.createUser");
    assert_eq!(result.entity_type, "java.method");
    assert_eq!(result.signature, "createUser(String, String)");
    assert!(!result.witnesses.is_empty(), "Should have witnesses");
    assert!(
        result.witnesses[0].start_line > 0,
        "Witness should have line number"
    );
}

#[test]
fn test_explain_class() {
    let engine = build_engine();
    let result = engine
        .explain("com.example.model.User")
        .expect("explain failed");

    assert_eq!(result.entity_type, "java.class");
}

#[test]
fn test_explain_interface() {
    let engine = build_engine();
    let result = engine
        .explain("com.example.repository.UserRepository")
        .expect("explain failed");

    assert_eq!(result.entity_type, "java.interface");
}

#[test]
fn test_explain_short_name() {
    let engine = build_engine();
    // Should resolve short name to FQN
    let result = engine.explain("UserService").expect("explain failed");
    assert!(result.symbol.ends_with("UserService"));
}

#[test]
fn test_explain_not_found() {
    let engine = build_engine();
    let result = engine.explain("com.example.NonExistent");
    assert!(result.is_err());
}

#[test]
fn test_callees_delete_user() {
    let engine = build_engine();
    let result = engine
        .callees("com.example.service.UserService.deleteUser")
        .expect("callees failed");

    assert!(
        !result.relations.is_empty(),
        "deleteUser should have callees"
    );

    let callee_fqns: Vec<_> = result.relations.iter().map(|r| r.target.as_str()).collect();
    assert!(
        callee_fqns.iter().any(|f| f.contains("getUser")),
        "deleteUser should call getUser. Found: {callee_fqns:?}"
    );
}

#[test]
fn test_callers_returns_results() {
    let engine = build_engine();
    // getUser is called by deleteUser
    let result = engine
        .callers("com.example.service.UserService.getUser")
        .expect("callers failed");

    let caller_fqns: Vec<_> = result.relations.iter().map(|r| r.target.as_str()).collect();
    assert!(
        caller_fqns.iter().any(|f| f.contains("deleteUser")),
        "getUser should be called by deleteUser. Found: {caller_fqns:?}"
    );
}

#[test]
fn test_explain_has_callees_as_fqn() {
    let engine = build_engine();
    let result = engine
        .explain("com.example.service.UserService.createUser")
        .expect("explain failed");

    // Callees should be FQNs, not just labels
    for callee in &result.callees {
        assert!(callee.contains('.'), "Callee should be FQN, got: {callee}");
    }
}

// --- Overview ---

#[test]
fn test_overview() {
    let engine = build_engine();
    let result = engine.overview().expect("overview failed");

    assert!(result.total_entities > 0);
    assert!(result.total_relations > 0);
    assert!(result.total_files > 0);
    assert!(!result.entities_by_type.is_empty());
    assert!(!result.packages.is_empty());

    let type_names: Vec<_> = result
        .entities_by_type
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(type_names.contains(&"java.class"));
    assert!(type_names.contains(&"java.method"));
}

// --- Search ---

#[test]
fn test_search_by_name() {
    let engine = build_engine();
    let result = engine.search("User", None, 50).expect("search failed");

    assert!(
        result.total_matches > 0,
        "Should find User-related entities"
    );
    assert!(
        result.matches.iter().any(|m| m.fqn.contains("UserService")),
        "Should find UserService"
    );
}

#[test]
fn test_search_with_type_filter() {
    let engine = build_engine();
    let result = engine
        .search("User", Some("interface"), 50)
        .expect("search failed");

    for m in &result.matches {
        assert_eq!(
            m.entity_type, "java.interface",
            "Should only return interfaces"
        );
    }
}

// --- Feature ---

#[test]
fn test_feature_analysis() {
    let engine = build_engine();
    let result = engine.feature("User").expect("feature failed");

    assert!(
        !result.matched_classes.is_empty(),
        "Should find User classes"
    );
    assert!(
        result
            .matched_classes
            .iter()
            .any(|c| c.fqn.contains("UserService")),
        "Should include UserService"
    );
    assert!(result.total_methods > 0);
}

#[test]
fn test_feature_not_found() {
    let engine = build_engine();
    let result = engine.feature("NonExistentFeature");
    assert!(result.is_err());
}

// --- Impact ---

#[test]
fn test_impact_analysis() {
    let engine = build_engine();
    let result = engine
        .impact("com.example.service.UserService.getUser", 3)
        .expect("impact failed");

    assert_eq!(
        result.changed_symbol,
        "com.example.service.UserService.getUser"
    );
    // getUser is called by deleteUser
    assert!(
        result
            .direct_impacts
            .iter()
            .any(|i| i.fqn.contains("deleteUser")),
        "getUser change should impact deleteUser. Found: {:?}",
        result
            .direct_impacts
            .iter()
            .map(|i| &i.fqn)
            .collect::<Vec<_>>()
    );
}

// --- Unknowns ---

#[test]
fn test_unknowns() {
    let engine = build_engine();
    let result = engine.unknowns(None).expect("unknowns failed");
    // Should return without error; the fixture has no low-confidence entities
    assert_eq!(result.scope, "all");
}

// --- Annotate ---

#[test]
fn test_annotate_and_explain() {
    let engine = build_engine();

    // Annotate a method
    engine
        .annotate_by_fqn(
            "com.example.service.UserService.createUser",
            specgraphen_model::SemanticAnnotation {
                intent: Some("Create a new user with validation".to_string()),
                behavior: Some(
                    "Validates name and email, creates User, saves via repository".to_string(),
                ),
                preconditions: vec!["name must not be null or empty".to_string()],
                postconditions: vec!["user is persisted".to_string()],
                invariants: Vec::new(),
                side_effects: vec!["writes to repository".to_string()],
                error_behavior: Some("throws ValidationException on invalid input".to_string()),
            },
        )
        .expect("annotate failed");

    // Now explain should return the annotation
    let result = engine
        .explain("com.example.service.UserService.createUser")
        .expect("explain failed");

    assert!(
        result.intent.is_some(),
        "Should have intent after annotation"
    );
    assert_eq!(
        result.intent.unwrap().value,
        "Create a new user with validation"
    );
    assert!(result.behavior.is_some());
    assert!(!result.preconditions.is_empty());
    assert!(!result.side_effects.is_empty());
    assert!(result.error_behavior.is_some());
}
