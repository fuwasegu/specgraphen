//! # specgraphen-topology
//!
//! Pure topological-data-analysis (TDA) layer for recovering *stable domain
//! clusters* from a lifted Higher Graphen [`SpaceData`].
//!
//! The mathematical core of persistent homology — Z2 chain reduction, Betti
//! numbers, persistence intervals — already lives in
//! `higher-graphen-structure::topology`. This crate does **not** reimplement
//! it. Instead it supplies the parts HG has no opinion about:
//!
//! 1. A **distance** between code entities (MVP: call-graph hop distance).
//! 2. A **Vietoris-Rips filtration** built from that distance: point pairs at
//!    distance `d <= eps` become 1-cells, swept over increasing `eps`.
//! 3. **Delegation** of the persistence computation to HG via
//!    [`summarize_filtration_with_options`].
//! 4. Reading the H0 result back into **stable clusters**, mapping persistence
//!    lifetime to a `confidence`, and measuring how far each cluster drifts
//!    from the human-drawn package boundaries.
//!
//! This follows SPECIFICATION3 §0/§1: the numerical core is isolated from I/O
//! and from the AST, exactly as `specgraphen-logic` is. There is no MCP/CLI
//! surface here — callers wire that up.
//!
//! ## Why a separate complex
//!
//! The VR complex has its own dimensions and boundary system (0-cells are
//! points, 1-cells are proximity edges) that is unrelated to the lifted
//! incidence graph. So we build a dedicated [`InMemorySpaceStore`] for it
//! rather than reusing the one inside [`SpaceData`].
//!
//! ## Scope (MVP, per SPECIFICATION3 §1.6)
//!
//! We build point-pair 1-cells only. That yields H0 (stable clusters, the
//! primary signal) and births of H1 loops (cyclic dependency signal). We do
//! **not** add 2-cells, so H1 features never die — `open_hole_count` reports
//! long-lived loops but their death is not tracked. This keeps the complex
//! size at `O(points^2)` edges instead of `O(points^3)` triangles. For large
//! graphs the point set should be coarsened upstream before calling in (see
//! the note on [`call_hop_distance_matrix`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use higher_graphen_core::Id;
use higher_graphen_structure::space::{Cell, ComplexType, InMemorySpaceStore, Space};
use higher_graphen_structure::topology::{
    summarize_filtration_with_options, ConnectedComponentSummary, FiltrationStage,
    PersistenceOptions, PersistenceSummary,
};
use specgraphen_model::SpaceData;

/// Relation type emitted by `specgraphen-lift` for a method call edge.
const CALL_RELATION_TYPE: &str = "java.calls";

/// Errors surfaced while building a filtration or delegating to HG topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyError {
    /// The point set was empty, so no complex could be constructed.
    NoPoints,
    /// A Higher Graphen identifier or structural insertion was rejected.
    Hg(String),
}

impl std::fmt::Display for TopologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPoints => write!(f, "distance matrix has no points"),
            Self::Hg(reason) => write!(f, "higher-graphen rejected the complex: {reason}"),
        }
    }
}

impl std::error::Error for TopologyError {}

impl From<higher_graphen_core::CoreError> for TopologyError {
    fn from(error: higher_graphen_core::CoreError) -> Self {
        Self::Hg(error.to_string())
    }
}

/// A single code entity that participates in the analysis.
///
/// `cell_id` and `fqn` are passed straight through from the source
/// [`SpaceData`] so that downstream witnesses resolve back to the original
/// cell provenance without this crate owning any witness data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    /// Stable id of the source dimension-0 cell.
    pub cell_id: String,
    /// Fully-qualified name, when the source space records one for the cell.
    pub fqn: Option<String>,
}

/// Symmetric pairwise distance over a fixed, ordered set of [`Point`]s.
///
/// Distances are `f64`; an unreachable pair carries [`f64::INFINITY`] and
/// therefore never produces a Vietoris-Rips edge. The diagonal is zero.
#[derive(Debug, Clone)]
pub struct DistanceMatrix {
    points: Vec<Point>,
    /// Row-major `len * len`; `distances[i * len + j]` is `d(i, j)`.
    distances: Vec<f64>,
}

impl DistanceMatrix {
    /// Builds a matrix from points and a symmetric closure `metric(i, j)`.
    ///
    /// `metric` is only queried for `i < j`; the diagonal is forced to zero
    /// and the lower triangle mirrors the upper triangle.
    #[must_use]
    pub fn from_metric(points: Vec<Point>, mut metric: impl FnMut(usize, usize) -> f64) -> Self {
        let len = points.len();
        let mut distances = vec![f64::INFINITY; len * len];
        for i in 0..len {
            distances[i * len + i] = 0.0;
            for j in (i + 1)..len {
                let d = metric(i, j);
                distances[i * len + j] = d;
                distances[j * len + i] = d;
            }
        }
        Self { points, distances }
    }

    /// Number of points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// True when there are no points.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The ordered point set; row/column `i` corresponds to `points()[i]`.
    #[must_use]
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Distance between points `i` and `j`.
    #[must_use]
    pub fn get(&self, i: usize, j: usize) -> f64 {
        self.distances[i * self.points.len() + j]
    }

    /// Element-wise minimum fusion with another matrix over the *same* points.
    ///
    /// This is the seam where additional metrics (data-sharing, type-coupling,
    /// co-change) compose into a single distance, kept as a free function per
    /// SPECIFICATION3 §1.2. MVP ships only [`call_hop_distance_matrix`]; this
    /// combinator exists so adding a metric never reshapes the public API.
    ///
    /// Panics if the matrices disagree on point count.
    #[must_use]
    pub fn min_combine(mut self, other: &DistanceMatrix) -> Self {
        assert_eq!(
            self.points.len(),
            other.points.len(),
            "min_combine requires matrices over the same points"
        );
        for (lhs, rhs) in self.distances.iter_mut().zip(other.distances.iter()) {
            *lhs = lhs.min(*rhs);
        }
        self
    }
}

/// Builds the MVP distance matrix: **undirected call-graph hop distance**.
///
/// Points are the dimension-0 cells of `space`. Each `java.calls` incidence is
/// treated as an undirected unit-length edge; the distance between two points
/// is the number of hops on the shortest such path, or [`f64::INFINITY`] when
/// no path exists. This is the single, simple, verifiable indicator
/// SPECIFICATION3 §1.6 asks us to start from before composing metrics.
///
/// Complexity note: this is `O(V * (V + E))` (BFS from every point). For large
/// spaces the caller should coarsen the point set first (e.g. collapse to
/// class-level cells, or restrict to a subgraph) before building the matrix —
/// the Rips edge count is then `O(V^2)`.
#[must_use]
pub fn call_hop_distance_matrix(space: &SpaceData) -> DistanceMatrix {
    let points: Vec<Point> = space
        .cells
        .iter()
        .filter(|cell| cell.dimension == 0)
        .map(|cell| Point {
            cell_id: cell.id.to_string(),
            fqn: fqn_for_cell(space, cell.id.as_str()),
        })
        .collect();

    let index_of: HashMap<&str, usize> = points
        .iter()
        .enumerate()
        .map(|(i, p)| (p.cell_id.as_str(), i))
        .collect();

    // Undirected adjacency over call edges only.
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); points.len()];
    for incidence in &space.incidences {
        if incidence.relation_type != CALL_RELATION_TYPE {
            continue;
        }
        let (from, to) = (
            index_of.get(incidence.from_cell_id.as_str()),
            index_of.get(incidence.to_cell_id.as_str()),
        );
        if let (Some(&from), Some(&to)) = (from, to) {
            if from != to {
                adjacency[from].push(to);
                adjacency[to].push(from);
            }
        }
    }

    let hops = all_pairs_hop_distance(&adjacency);
    DistanceMatrix::from_metric(points, |i, j| hops[i][j])
}

/// BFS hop distance from every vertex; unreachable pairs are `INFINITY`.
fn all_pairs_hop_distance(adjacency: &[Vec<usize>]) -> Vec<Vec<f64>> {
    let len = adjacency.len();
    let mut out = vec![vec![f64::INFINITY; len]; len];
    for source in 0..len {
        let mut depth = vec![usize::MAX; len];
        depth[source] = 0;
        let mut queue = VecDeque::from([source]);
        while let Some(node) = queue.pop_front() {
            for &next in &adjacency[node] {
                if depth[next] == usize::MAX {
                    depth[next] = depth[node] + 1;
                    queue.push_back(next);
                }
            }
        }
        for (target, d) in depth.into_iter().enumerate() {
            if d != usize::MAX {
                out[source][target] = d as f64;
            }
        }
    }
    out
}

/// Looks up the FQN recorded for a cell id, if the source space has one.
fn fqn_for_cell(space: &SpaceData, cell_id: &str) -> Option<String> {
    space
        .entities
        .iter()
        .find(|entity| entity.cell_id == cell_id)
        .map(|entity| entity.fqn.clone())
}

/// Tuning for cluster extraction from the persistence summary.
#[derive(Debug, Clone, Copy)]
pub struct DomainClusterOptions {
    /// Minimum H0 lifetime (in filtration steps) for a component to count as a
    /// *stable* cluster. Larger values keep only long-lived domain blobs and
    /// discard short-lived noise merges. Passed through to HG as
    /// `PersistenceOptions::min_lifetime_stages`.
    pub min_lifetime_stages: usize,
}

impl Default for DomainClusterOptions {
    fn default() -> Self {
        // A born-at-stage-0 point that merges at the very first edge level has
        // lifetime 1; that is the noise floor. Requiring lifetime >= 2 keeps
        // only components that survived at least one merge round, i.e. genuine
        // clusters rather than individual entities. Callers raise this to
        // demand stronger separation.
        Self {
            min_lifetime_stages: 2,
        }
    }
}

/// One recovered stable domain cluster (an H0 connected component at the cut).
#[derive(Debug, Clone, PartialEq)]
pub struct DomainCluster {
    /// Source cell ids of the entities in this cluster.
    pub member_cell_ids: Vec<String>,
    /// Their FQNs, for the members that carry one (witness passthrough).
    pub member_fqns: Vec<String>,
    /// Distinct package prefixes the members fall under (see
    /// [`package_of`]). Length `> 1` means the cluster straddles packages.
    pub packages: Vec<String>,
    /// Persistence lifetime (filtration steps) of the H0 interval that this
    /// component realizes; the basis for `confidence`.
    pub persistence_lifetime: usize,
    /// Lifetime mapped into `[0, 1]` (see [`lifetime_to_confidence`]).
    pub confidence: f64,
}

impl DomainCluster {
    /// True when the cluster spans more than one package — the "this cluster
    /// crosses N packages" drift signal of SPECIFICATION3 §1.4.
    #[must_use]
    pub fn crosses_package_boundary(&self) -> bool {
        self.packages.len() > 1
    }
}

/// Result of a domain-boundary analysis.
#[derive(Debug, Clone)]
pub struct DomainAnalysis {
    /// Stable clusters, ordered by descending persistence then by first member
    /// id for determinism.
    pub clusters: Vec<DomainCluster>,
    /// Zero-based filtration stage the clusters were read from.
    pub cut_stage_index: usize,
    /// The raw HG persistence summary (Betti numbers, every interval, per-stage
    /// topology). Passed through untouched so callers can inspect H1 loop
    /// births (`open_hole_count`) or render full persistence diagrams.
    pub persistence: PersistenceSummary,
}

impl DomainAnalysis {
    /// Clusters whose members come from more than one package.
    #[must_use]
    pub fn boundary_drifts(&self) -> Vec<&DomainCluster> {
        self.clusters
            .iter()
            .filter(|cluster| cluster.crosses_package_boundary())
            .collect()
    }
}

/// Runs the full TDA pipeline: VR filtration → HG persistence → stable
/// clusters with package-boundary drift.
///
/// Errors only on a structurally impossible input (no points) or if HG rejects
/// the constructed complex — both indicate a bug here rather than a recoverable
/// condition, so no fallback is attempted (project policy).
pub fn detect_domain_clusters(
    distances: &DistanceMatrix,
    options: DomainClusterOptions,
) -> Result<DomainAnalysis, TopologyError> {
    if distances.is_empty() {
        return Err(TopologyError::NoPoints);
    }

    let complex = VrComplex::build(distances)?;
    let summary = summarize_filtration_with_options(
        &complex.store,
        &complex.complex_id,
        &complex.stages,
        PersistenceOptions::new().with_min_lifetime_stages(options.min_lifetime_stages),
    )?;

    let cut_stage_index = choose_cut_stage(&summary);
    let clusters = extract_clusters(distances, &summary, cut_stage_index);

    Ok(DomainAnalysis {
        clusters,
        cut_stage_index,
        persistence: summary,
    })
}

/// A Vietoris-Rips complex and its cumulative filtration over a dedicated store.
struct VrComplex {
    store: InMemorySpaceStore,
    complex_id: Id,
    stages: Vec<FiltrationStage>,
}

impl VrComplex {
    /// Constructs the VR complex.
    ///
    /// Cells: one 0-cell per point (reusing the source cell id so witnesses map
    /// back), plus one 1-cell per finite-distance pair. The filtration sweeps
    /// the distinct finite distances in ascending order; stage 0 is all points,
    /// and each later stage adds every edge whose distance `<= eps`. Because
    /// all points are present from stage 0, HG's cumulative + boundary-closure
    /// stage validation is satisfied for every edge.
    fn build(distances: &DistanceMatrix) -> Result<Self, TopologyError> {
        let space_id = Id::new("vr.space")?;
        let mut store = InMemorySpaceStore::new();
        store.insert_space(Space::new(space_id.clone(), "Vietoris-Rips complex"))?;

        // Points first: a cell's boundary cells must already exist on insert.
        let mut point_ids = Vec::with_capacity(distances.len());
        for point in distances.points() {
            let id = Id::new(point.cell_id.clone())?;
            store.insert_cell(Cell::new(id.clone(), space_id.clone(), 0, "vr.point"))?;
            point_ids.push(id);
        }

        // Group edges by threshold so each filtration stage adds a full level.
        // Keyed by ordered f64 bits for a deterministic ascending sweep.
        let mut edges_by_threshold: BTreeMap<u64, Vec<Id>> = BTreeMap::new();
        let mut all_edge_ids = Vec::new();
        let len = distances.len();
        for i in 0..len {
            for j in (i + 1)..len {
                let d = distances.get(i, j);
                if !d.is_finite() {
                    continue;
                }
                let edge_id = Id::new(format!("vr.edge.{i}-{j}"))?;
                store.insert_cell(
                    Cell::new(edge_id.clone(), space_id.clone(), 1, "vr.edge")
                        .with_boundary_cell(point_ids[i].clone())
                        .with_boundary_cell(point_ids[j].clone()),
                )?;
                edges_by_threshold
                    .entry(d.to_bits())
                    .or_default()
                    .push(edge_id.clone());
                all_edge_ids.push(edge_id);
            }
        }

        // Complex holds every cell across the whole filtration.
        let mut complex_cells = point_ids.clone();
        complex_cells.extend(all_edge_ids.iter().cloned());
        let complex = store.construct_complex(
            Id::new("vr.complex")?,
            space_id.clone(),
            "Vietoris-Rips filtration",
            ComplexType::SimplicialComplex,
            complex_cells,
            [],
        )?;

        // Cumulative stages: stage 0 = all points; then one stage per distinct
        // threshold, each a superset of the previous.
        let mut cumulative: Vec<Id> = point_ids;
        let mut stages = vec![FiltrationStage::new(
            Id::new("vr.stage.0")?,
            cumulative.clone(),
        )];
        for (step, (_, edge_ids)) in edges_by_threshold.into_iter().enumerate() {
            cumulative.extend(edge_ids);
            stages.push(FiltrationStage::new(
                Id::new(format!("vr.stage.{}", step + 1))?,
                cumulative.clone(),
            ));
        }

        Ok(Self {
            store,
            complex_id: complex.id,
            stages,
        })
    }
}

/// Chooses the filtration stage to read clusters from.
///
/// The survivor count is the number of *persistent* H0 intervals — those HG
/// kept after applying the lifetime threshold (`persistent_intervals`). We cut
/// at the **latest** stage whose live component count equals that survivor
/// count: at that scale the persistent clusters have absorbed all their
/// short-lived noise but have not yet merged into each other, so each cluster
/// is grown as large as it gets before a merge. If no stage matches exactly
/// (e.g. an aggressive threshold), we fall back to the last stage, which is
/// always well defined.
fn choose_cut_stage(summary: &PersistenceSummary) -> usize {
    let last_stage_index = summary.stages.len().saturating_sub(1);
    let survivor_count = summary
        .persistent_intervals
        .iter()
        .filter(|interval| interval.dimension == 0)
        .count();

    summary
        .stages
        .iter()
        .rev()
        .find(|stage| stage.topology.component_count == survivor_count)
        .map_or(last_stage_index, |stage| stage.stage_index)
}

/// Reads connected components at the cut stage into [`DomainCluster`]s, tagging
/// each with the persistence lifetime of the H0 interval it realizes.
fn extract_clusters(
    distances: &DistanceMatrix,
    summary: &PersistenceSummary,
    cut_stage_index: usize,
) -> Vec<DomainCluster> {
    let Some(stage) = summary.stages.get(cut_stage_index) else {
        return Vec::new();
    };
    let last_stage_index = summary.stages.len().saturating_sub(1);

    // Per-point FQN lookup keyed by source cell id.
    let fqn_by_cell: HashMap<&str, &str> = distances
        .points()
        .iter()
        .filter_map(|p| p.fqn.as_deref().map(|fqn| (p.cell_id.as_str(), fqn)))
        .collect();

    let lifetime_by_generator = h0_lifetime_by_generator(summary, last_stage_index);
    let min_lifetime = summary.options.min_lifetime_stages;

    let mut clusters: Vec<DomainCluster> = stage
        .topology
        .connected_components
        .iter()
        .filter_map(|component| {
            let persistence_lifetime = component_h0_lifetime(component, &lifetime_by_generator);
            if persistence_lifetime < min_lifetime {
                return None;
            }

            let member_cell_ids: Vec<String> = component
                .vertex_cell_ids
                .iter()
                .map(ToString::to_string)
                .collect();

            let member_fqns: Vec<String> = member_cell_ids
                .iter()
                .filter_map(|id| fqn_by_cell.get(id.as_str()).map(|fqn| (*fqn).to_owned()))
                .collect();

            let packages = distinct_packages(&member_fqns);

            Some(DomainCluster {
                member_cell_ids,
                member_fqns,
                packages,
                persistence_lifetime,
                confidence: lifetime_to_confidence(persistence_lifetime, last_stage_index),
            })
        })
        .collect();

    // Descending persistence, then first member id, for stable output.
    clusters.sort_by(|a, b| {
        b.persistence_lifetime
            .cmp(&a.persistence_lifetime)
            .then_with(|| a.member_cell_ids.cmp(&b.member_cell_ids))
    });
    clusters
}

fn h0_lifetime_by_generator(
    summary: &PersistenceSummary,
    last_stage_index: usize,
) -> HashMap<&str, usize> {
    summary
        .intervals
        .iter()
        .filter(|interval| interval.dimension == 0)
        .filter_map(|interval| {
            interval
                .generator_cell_ids
                .first()
                .map(|gen| (gen.as_str(), interval.lifetime_stages(last_stage_index)))
        })
        .collect()
}

fn component_h0_lifetime(
    component: &ConnectedComponentSummary,
    lifetime_by_generator: &HashMap<&str, usize>,
) -> usize {
    let representative = component.representative_cell_id.as_str();
    debug_assert!(
        lifetime_by_generator.contains_key(representative),
        "HG H0 interval generators must match cut-stage component representatives"
    );
    lifetime_by_generator
        .get(representative)
        .copied()
        .expect("HG H0 interval generator missing for cut-stage component representative")
}

/// Distinct sorted package prefixes for a set of FQNs.
fn distinct_packages(fqns: &[String]) -> Vec<String> {
    fqns.iter()
        .map(|fqn| package_of(fqn))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The package of an FQN: everything up to the last `.`, else the whole string.
///
/// `com.example.svc.UserService` → `com.example.svc`. This is the human-drawn
/// boundary the cluster is compared against (SPECIFICATION3 §1.4).
#[must_use]
pub fn package_of(fqn: &str) -> String {
    match fqn.rfind('.') {
        Some(idx) => fqn[..idx].to_owned(),
        None => fqn.to_owned(),
    }
}

/// Maps an H0 persistence lifetime to a confidence in `[0, 1]`.
///
/// A feature that survives the whole filtration (lifetime == total steps) maps
/// to `1.0`; an instantaneous one to `0.0`. `max_lifetime` is the number of
/// filtration steps (the last stage index), so confidence is the fraction of
/// the sweep the cluster stayed coherent.
#[must_use]
pub fn lifetime_to_confidence(lifetime: usize, max_lifetime: usize) -> f64 {
    if max_lifetime == 0 {
        // A single-stage filtration cannot separate persistence; everything
        // present is trivially "fully persistent".
        return 1.0;
    }
    (lifetime as f64 / max_lifetime as f64).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests;
