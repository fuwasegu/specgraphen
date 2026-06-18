//! `domain_clusters` query: TDA-based domain boundary detection.
//!
//! Thin query wrapper over the `specgraphen-topology` crate (which delegates
//! persistent homology to HG). Surfaces stable domain clusters and their
//! divergence from the existing package layout (SPECIFICATION3 §1).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;
use specgraphen_topology::{
    call_hop_distance_matrix, detect_domain_clusters, DomainClusterOptions,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct DomainClustersResult {
    pub cluster_count: usize,
    /// Clusters that span more than one package (refactor candidates).
    pub boundary_drift_count: usize,
    pub clusters: Vec<DomainClusterView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DomainClusterView {
    pub member_fqns: Vec<String>,
    pub packages: Vec<String>,
    pub confidence: f64,
    pub persistence_lifetime: usize,
    pub crosses_package_boundary: bool,
}

pub fn domain_clusters(
    space_data: &SpaceData,
    min_lifetime: usize,
) -> Result<DomainClustersResult> {
    let matrix = call_hop_distance_matrix(space_data);
    if matrix.is_empty() {
        return Ok(DomainClustersResult {
            cluster_count: 0,
            boundary_drift_count: 0,
            clusters: Vec::new(),
        });
    }

    let analysis = detect_domain_clusters(
        &matrix,
        DomainClusterOptions {
            min_lifetime_stages: min_lifetime,
        },
    )
    .map_err(|e| anyhow::anyhow!("topology analysis failed: {e:?}"))?;

    let boundary_drift_count = analysis.boundary_drifts().len();
    let clusters = analysis
        .clusters
        .iter()
        .map(|c| DomainClusterView {
            member_fqns: c.member_fqns.clone(),
            packages: c.packages.clone(),
            confidence: c.confidence,
            persistence_lifetime: c.persistence_lifetime,
            crosses_package_boundary: c.crosses_package_boundary(),
        })
        .collect();

    Ok(DomainClustersResult {
        cluster_count: analysis.clusters.len(),
        boundary_drift_count,
        clusters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lift_fixture;

    #[test]
    fn runs_on_fixture_and_is_self_consistent() {
        let sd = lift_fixture();
        let r = domain_clusters(&sd, 2).unwrap();
        assert_eq!(r.cluster_count, r.clusters.len());
        assert!(r.boundary_drift_count <= r.cluster_count);
    }
}
