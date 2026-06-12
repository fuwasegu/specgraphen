use std::path::PathBuf;

use specgraphen_lift::{JavaLifter, LiftConfig};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/simple-project")
}

#[test]
fn test_lift_extracts_entities() {
    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path(),
        space_id: "test-project".to_string(),
        space_label: "Test Project".to_string(),
        ..Default::default()
    };

    let result = lifter.lift(&config).expect("Lift failed");
    let data = &result.space_data;

    // Check we have entities
    assert!(!data.cells.is_empty(), "Should have extracted cells");
    assert!(!data.entities.is_empty(), "Should have entity records");

    // Check specific entity types exist
    let entity_types: Vec<_> = data.entities.iter().map(|e| &e.entity_type).collect();
    assert!(
        entity_types.contains(&&specgraphen_model::JavaEntityType::Package),
        "Should have Package entity"
    );
    assert!(
        entity_types.contains(&&specgraphen_model::JavaEntityType::Class),
        "Should have Class entity"
    );
    assert!(
        entity_types.contains(&&specgraphen_model::JavaEntityType::Interface),
        "Should have Interface entity"
    );
    assert!(
        entity_types.contains(&&specgraphen_model::JavaEntityType::Method),
        "Should have Method entity"
    );
    assert!(
        entity_types.contains(&&specgraphen_model::JavaEntityType::Field),
        "Should have Field entity"
    );
    assert!(
        entity_types.contains(&&specgraphen_model::JavaEntityType::Constructor),
        "Should have Constructor entity"
    );
}

#[test]
fn test_lift_extracts_known_classes() {
    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path(),
        space_id: "test-project".to_string(),
        space_label: "Test Project".to_string(),
        ..Default::default()
    };

    let result = lifter.lift(&config).expect("Lift failed");
    let fqn_keys: Vec<_> = result.space_data.fqn_to_cell_id.keys().cloned().collect();

    // Check expected FQNs
    let expected_fqns = [
        "com.example.model",
        "com.example.model.User",
        "com.example.service.UserService",
        "com.example.service.NotificationService",
        "com.example.repository.UserRepository",
        "com.example.exception.ValidationException",
    ];

    for expected in &expected_fqns {
        assert!(
            fqn_keys.iter().any(|k| k == expected),
            "Missing FQN: {expected}. Found: {fqn_keys:?}"
        );
    }
}

#[test]
fn test_lift_extracts_methods() {
    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path(),
        space_id: "test-project".to_string(),
        space_label: "Test Project".to_string(),
        ..Default::default()
    };

    let result = lifter.lift(&config).expect("Lift failed");
    let fqn_keys: Vec<_> = result.space_data.fqn_to_cell_id.keys().cloned().collect();

    let expected_methods = [
        "com.example.service.UserService.createUser",
        "com.example.service.UserService.getUser",
        "com.example.service.UserService.deleteUser",
        "com.example.model.User.getName",
        "com.example.model.User.getEmail",
        "com.example.repository.UserRepository.save",
        "com.example.repository.UserRepository.findById",
    ];

    for expected in &expected_methods {
        assert!(
            fqn_keys.iter().any(|k| k == expected),
            "Missing method FQN: {expected}"
        );
    }
}

#[test]
fn test_lift_extracts_relations() {
    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path(),
        space_id: "test-project".to_string(),
        space_label: "Test Project".to_string(),
        ..Default::default()
    };

    let result = lifter.lift(&config).expect("Lift failed");
    let data = &result.space_data;

    // Should have incidences (relations)
    assert!(
        !data.incidences.is_empty(),
        "Should have extracted relations"
    );

    // Check relation types
    let rel_types: Vec<_> = data
        .incidences
        .iter()
        .map(|i| i.relation_type.as_str())
        .collect();

    assert!(
        rel_types.contains(&"java.contained_in"),
        "Should have ContainedIn relations"
    );
}

#[test]
fn test_lift_all_cells_have_provenance() {
    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path(),
        space_id: "test-project".to_string(),
        space_label: "Test Project".to_string(),
        ..Default::default()
    };

    let result = lifter.lift(&config).expect("Lift failed");

    for cell in &result.space_data.cells {
        assert!(
            cell.provenance.is_some(),
            "Cell {} ({}) should have provenance",
            cell.id.as_str(),
            cell.label.as_deref().unwrap_or("?")
        );
    }
}

#[test]
fn test_lift_space_references_all_cells() {
    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path(),
        space_id: "test-project".to_string(),
        space_label: "Test Project".to_string(),
        ..Default::default()
    };

    let result = lifter.lift(&config).expect("Lift failed");
    let data = &result.space_data;

    assert_eq!(
        data.space.cell_ids.len(),
        data.cells.len(),
        "Space cell_ids should match cells count"
    );
    assert_eq!(
        data.space.incidence_ids.len(),
        data.incidences.len(),
        "Space incidence_ids should match incidences count"
    );
}

#[test]
fn test_lift_diagnostics_reports_unresolved() {
    let mut lifter = JavaLifter::new().expect("Failed to create lifter");
    let config = LiftConfig {
        root_path: fixture_path(),
        space_id: "test-project".to_string(),
        space_label: "Test Project".to_string(),
        ..Default::default()
    };

    let result = lifter.lift(&config).expect("Lift failed");

    // Some calls will be unresolved (e.g., System.out.println, String methods)
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| matches!(d.severity, specgraphen_lift::DiagnosticSeverity::Warning))
        .collect();

    // We expect some unresolved references (stdlib calls)
    assert!(
        !warnings.is_empty(),
        "Should have unresolved reference warnings for stdlib calls"
    );
}
