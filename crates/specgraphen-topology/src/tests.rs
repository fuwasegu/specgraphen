use std::collections::HashMap;

use higher_graphen_core::Id;
use higher_graphen_structure::space::{Cell, Incidence, IncidenceOrientation, Space};
use specgraphen_model::derivation::DerivationSource;
use specgraphen_model::witness::WitnessInfo;
use specgraphen_model::{EntityRecord, JavaEntityType, JavaRelationType, SpaceData};

use super::*;

fn id(value: &str) -> Id {
    Id::new(value).expect("valid id")
}

fn witness() -> WitnessInfo {
    WitnessInfo {
        file: "Test.java".to_owned(),
        start_line: 1,
        end_line: 1,
        start_col: 0,
        end_col: 0,
        derivation_source: DerivationSource::TreeSitter,
    }
}

/// Builds a `SpaceData` from method points and undirected `java.calls` edges.
///
/// `points` is `(cell_id, fqn)`; an entity record is created so FQN
/// passthrough and package-drift can be exercised. `calls` is `(from, to)`.
fn space_with_calls(points: &[(&str, &str)], calls: &[(&str, &str)]) -> SpaceData {
    let space = Space::new(id("space-a"), "Test space");
    let cells: Vec<Cell> = points
        .iter()
        .map(|(cell_id, _)| Cell::new(id(cell_id), id("space-a"), 0, "java.method"))
        .collect();
    let entities: Vec<EntityRecord> = points
        .iter()
        .map(|(cell_id, fqn)| EntityRecord {
            fqn: (*fqn).to_owned(),
            cell_id: (*cell_id).to_owned(),
            entity_type: JavaEntityType::Method,
            label: (*fqn).to_owned(),
            witness: witness(),
        })
        .collect();
    let incidences: Vec<Incidence> = calls
        .iter()
        .enumerate()
        .map(|(index, (from, to))| {
            Incidence::new(
                id(&format!("call-{index}")),
                id("space-a"),
                id(from),
                id(to),
                JavaRelationType::Calls.relation_type_str(),
                IncidenceOrientation::Directed,
            )
        })
        .collect();

    SpaceData::new(
        space,
        cells,
        incidences,
        entities,
        Vec::new(),
        HashMap::new(),
    )
}

fn two_disconnected_clique_matrix() -> DistanceMatrix {
    let space = space_with_calls(
        &[
            ("A", "com.order.A"),
            ("B", "com.order.B"),
            ("C", "com.order.C"),
            ("D", "com.billing.D"),
            ("E", "com.billing.E"),
            ("F", "com.billing.F"),
        ],
        &[
            ("A", "B"),
            ("B", "C"),
            ("C", "A"),
            ("D", "E"),
            ("E", "F"),
            ("F", "D"),
        ],
    );
    call_hop_distance_matrix(&space)
}

#[test]
fn hop_distance_is_undirected_and_marks_unreachable() {
    // A -> B -> C is a directed chain; distance must ignore direction.
    let space = space_with_calls(
        &[
            ("com.a.A", "com.a.A.m"),
            ("com.a.B", "com.a.B.m"),
            ("com.a.C", "com.a.C.m"),
            ("com.z.Z", "com.z.Z.m"),
        ],
        &[("com.a.A", "com.a.B"), ("com.a.B", "com.a.C")],
    );
    let matrix = call_hop_distance_matrix(&space);
    assert_eq!(matrix.len(), 4);

    let idx = |fqn_prefix: &str| {
        matrix
            .points()
            .iter()
            .position(|p| p.cell_id == fqn_prefix)
            .expect("point present")
    };
    let (a, b, c, z) = (
        idx("com.a.A"),
        idx("com.a.B"),
        idx("com.a.C"),
        idx("com.z.Z"),
    );

    assert_eq!(matrix.get(a, b), 1.0);
    assert_eq!(matrix.get(b, a), 1.0, "distance is symmetric");
    assert_eq!(matrix.get(a, c), 2.0, "two hops A-B-C even though directed");
    assert_eq!(matrix.get(a, a), 0.0);
    assert!(
        matrix.get(a, z).is_infinite(),
        "Z is unreachable from the chain"
    );
}

#[test]
fn two_disconnected_cliques_yield_two_persistent_clusters() {
    // Triangle {A,B,C} and triangle {D,E,F}, no edge between them. Each clique
    // collapses to one component at eps=1 and the two never merge, so H0 keeps
    // exactly two open (forever-lived) components: two stable clusters.
    let matrix = two_disconnected_clique_matrix();
    let analysis =
        detect_domain_clusters(&matrix, DomainClusterOptions::default()).expect("analysis");

    assert_eq!(analysis.clusters.len(), 2, "two stable clusters");
    assert_eq!(analysis.persistence.open_component_count, 2);
    let last_stage_index = analysis.persistence.stages.len() - 1;
    let mut open_h0_lifetimes = analysis
        .persistence
        .intervals
        .iter()
        .filter(|interval| interval.dimension == 0 && interval.is_open())
        .map(|interval| interval.lifetime_stages(last_stage_index))
        .collect::<Vec<_>>();
    open_h0_lifetimes.sort_unstable();
    // HG v0.7.1 counts an open interval through the last stage
    // (`last_stage_index + 1 - birth`), so these two-stage clusters have
    // lifetime 2 and satisfy the default min_lifetime.
    assert_eq!(open_h0_lifetimes, vec![2, 2]);

    let mut members: Vec<Vec<String>> = analysis
        .clusters
        .iter()
        .map(|c| {
            let mut m = c.member_cell_ids.clone();
            m.sort();
            m
        })
        .collect();
    members.sort();
    assert_eq!(
        members,
        vec![
            vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
            vec!["D".to_owned(), "E".to_owned(), "F".to_owned()],
        ]
    );

    // Each clique is internally coherent and from a single package -> no drift.
    for cluster in &analysis.clusters {
        assert_eq!(cluster.packages.len(), 1);
        assert!(!cluster.crosses_package_boundary());
        assert_eq!(cluster.confidence, 1.0, "open interval is fully persistent");
    }
    assert!(analysis.boundary_drifts().is_empty());
}

#[test]
fn min_lifetime_filters_cut_stage_clusters() {
    let matrix = two_disconnected_clique_matrix();
    let analysis = detect_domain_clusters(
        &matrix,
        DomainClusterOptions {
            min_lifetime_stages: 3,
        },
    )
    .expect("analysis");

    // The cut-stage fallback is still the last stage, but each open H0 interval
    // has lifetime 2 under HG v0.7.1, so option (b) filters both clusters.
    assert_eq!(
        analysis.cut_stage_index,
        analysis.persistence.stages.len() - 1
    );
    assert_eq!(analysis.persistence.open_component_count, 2);
    assert!(analysis.clusters.is_empty());
}

#[test]
fn hg_h0_generators_match_cut_stage_component_representatives() {
    let matrix = two_disconnected_clique_matrix();
    let analysis =
        detect_domain_clusters(&matrix, DomainClusterOptions::default()).expect("analysis");

    let last_stage_index = analysis.persistence.stages.len() - 1;
    let lifetime_by_generator = h0_lifetime_by_generator(&analysis.persistence, last_stage_index);
    let stage = &analysis.persistence.stages[analysis.cut_stage_index];

    assert_eq!(stage.topology.connected_components.len(), 2);
    for component in &stage.topology.connected_components {
        assert!(
            lifetime_by_generator.contains_key(component.representative_cell_id.as_str()),
            "HG H0 generator id must match component representative {}",
            component.representative_cell_id
        );
    }
}

#[test]
fn connected_graph_collapses_to_a_single_cluster() {
    let space = space_with_calls(
        &[("A", "com.a.A"), ("B", "com.a.B"), ("C", "com.a.C")],
        &[("A", "B"), ("B", "C")],
    );
    let matrix = call_hop_distance_matrix(&space);
    let analysis =
        detect_domain_clusters(&matrix, DomainClusterOptions::default()).expect("analysis");

    assert_eq!(analysis.clusters.len(), 1);
    assert_eq!(analysis.clusters[0].member_cell_ids.len(), 3);
}

#[test]
fn cluster_spanning_two_packages_is_reported_as_drift() {
    // One connected clique whose members live in two different packages: the
    // recovered cluster crosses the human-drawn boundary.
    let space = space_with_calls(
        &[
            ("A", "com.order.A"),
            ("B", "com.order.B"),
            ("C", "com.shipping.C"),
        ],
        &[("A", "B"), ("B", "C"), ("C", "A")],
    );
    let matrix = call_hop_distance_matrix(&space);
    let analysis =
        detect_domain_clusters(&matrix, DomainClusterOptions::default()).expect("analysis");

    assert_eq!(analysis.clusters.len(), 1);
    let cluster = &analysis.clusters[0];
    assert!(cluster.crosses_package_boundary());
    assert_eq!(
        cluster.packages,
        vec!["com.order".to_owned(), "com.shipping".to_owned()]
    );
    assert_eq!(analysis.boundary_drifts().len(), 1);
}

#[test]
fn graduated_distances_track_a_merge_and_pick_the_two_cluster_cut() {
    // A hand-built distance matrix where two pairs are tight (d=1) and the gap
    // between the pairs is wide (d=3). The filtration has stages at eps in
    // {0, 1, 3}: at eps=1 two 2-point clusters form, at eps=3 they merge. This
    // exercises a real H0 death plus the cut-stage selection that the
    // collapsing unit-hop metric cannot produce on a connected graph.
    let points = vec![
        Point {
            cell_id: "p0".to_owned(),
            fqn: Some("com.left.P0".to_owned()),
        },
        Point {
            cell_id: "p1".to_owned(),
            fqn: Some("com.left.P1".to_owned()),
        },
        Point {
            cell_id: "p2".to_owned(),
            fqn: Some("com.right.P2".to_owned()),
        },
        Point {
            cell_id: "p3".to_owned(),
            fqn: Some("com.right.P3".to_owned()),
        },
    ];
    // tight pairs: (p0,p1) and (p2,p3) at 1.0; everything across the gap at 3.0.
    let matrix = DistanceMatrix::from_metric(points, |i, j| {
        let same_pair = (i / 2) == (j / 2);
        if same_pair {
            1.0
        } else {
            3.0
        }
    });

    let analysis = detect_domain_clusters(&matrix, DomainClusterOptions::default())
        .expect("analysis with graduated distances");

    // Stages: eps in {0,1,3} -> 3 stages (indices 0,1,2). Component counts go
    // 4 -> 2 -> 1. The cut is the latest 2-component stage (index 1).
    assert_eq!(analysis.persistence.stages.len(), 3);
    let component_counts: Vec<usize> = analysis
        .persistence
        .stages
        .iter()
        .map(|s| s.topology.component_count)
        .collect();
    assert_eq!(component_counts, vec![4, 2, 1]);
    assert_eq!(analysis.cut_stage_index, 1);

    assert_eq!(analysis.clusters.len(), 2);
    for cluster in &analysis.clusters {
        assert_eq!(cluster.member_cell_ids.len(), 2);
        // Each tight pair shares a package -> no drift here.
        assert!(!cluster.crosses_package_boundary());
    }

    // Both surviving clusters are born at stage 0; one merges (dies) at stage 2
    // and the other stays open. The persistent ones live >= 2 steps and map to
    // a confidence around 1.0 (2/2) for the open one and 1.0 (2/2) for the
    // closed one that dies at index 2 (lifetime 2 over max 2).
    let lifetimes: Vec<usize> = analysis
        .clusters
        .iter()
        .map(|c| c.persistence_lifetime)
        .collect();
    assert!(
        lifetimes.iter().all(|&l| l >= 1),
        "clusters persist across the gap"
    );
    assert!(analysis.clusters.iter().all(|c| c.confidence > 0.0));
}

#[test]
fn empty_point_set_is_an_error() {
    let space = space_with_calls(&[], &[]);
    let matrix = call_hop_distance_matrix(&space);
    assert!(matrix.is_empty());
    assert_eq!(
        detect_domain_clusters(&matrix, DomainClusterOptions::default()).unwrap_err(),
        TopologyError::NoPoints
    );
}

#[test]
fn package_and_confidence_helpers() {
    assert_eq!(package_of("com.example.svc.UserService"), "com.example.svc");
    assert_eq!(package_of("NoPackage"), "NoPackage");

    assert_eq!(lifetime_to_confidence(0, 4), 0.0);
    assert_eq!(lifetime_to_confidence(2, 4), 0.5);
    assert_eq!(lifetime_to_confidence(4, 4), 1.0);
    assert_eq!(lifetime_to_confidence(3, 0), 1.0, "single-stage edge case");
}

#[test]
fn persistence_summary_round_trips_through_serde() {
    // The summary is passed through to callers; confirm it stays serializable.
    let space = space_with_calls(&[("A", "com.a.A"), ("B", "com.a.B")], &[("A", "B")]);
    let matrix = call_hop_distance_matrix(&space);
    let analysis =
        detect_domain_clusters(&matrix, DomainClusterOptions::default()).expect("analysis");
    let json = serde_json::to_string(&analysis.persistence).expect("serialize");
    let _: PersistenceSummary = serde_json::from_str(&json).expect("deserialize");
}
