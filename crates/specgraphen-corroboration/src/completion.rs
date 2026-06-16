//! Obstruction-driven completion over unresolved references.
//!
//! Realizes DESIGN.md's obstruction → completion-candidate loop for the
//! `MissingMorphism` case: every unresolved call/construction recorded during
//! lift becomes a structured HG `Obstruction`, which HG translates into a
//! reviewable `CompletionCandidate`. We classify candidates by the confidence
//! HG carries through, giving an auto-accept / auto-reject / pending split
//! without a human reviewer (the project runs fully automatically).

use higher_graphen_core::{Confidence, Id, Provenance, Severity, SourceKind, SourceRef};
use higher_graphen_reasoning::completion::{
    detect_obstruction_completion_candidates, ObstructionCompletionInput,
};
use higher_graphen_reasoning::obstruction::{
    Obstruction, ObstructionExplanation, ObstructionType, RequiredResolution,
};
use specgraphen_model::SpaceData;

/// Confidence-based split of obstruction-driven completion candidates.
#[derive(Debug, Default, Clone)]
pub struct CompletionStats {
    /// Completion candidates HG derived from unresolved-reference obstructions.
    pub candidates: usize,
    /// Candidates whose confidence is at or above the high threshold.
    pub auto_accepted: usize,
    /// Candidates whose confidence is below the low threshold.
    pub auto_rejected: usize,
    /// Candidates between the thresholds, left for further evidence.
    pub pending: usize,
}

/// Translate unresolved references into HG completion candidates and classify
/// them by confidence. Returns an empty result when nothing is unresolved.
pub fn run_obstruction_completion(
    space_data: &SpaceData,
    high_threshold: f64,
    low_threshold: f64,
) -> anyhow::Result<CompletionStats> {
    if space_data.unresolved.is_empty() {
        return Ok(CompletionStats::default());
    }

    let space_id = space_data.space.id.clone();
    let mut obstructions = Vec::new();
    for (i, u) in space_data.unresolved.iter().enumerate() {
        let explanation = ObstructionExplanation::new(format!(
            "unresolved {}: {} -> {}",
            u.relation_type.relation_type_str(),
            u.from_fqn,
            u.target_text
        ))?;
        let mut obstruction = Obstruction::new(
            Id::new(format!("obstruction:unresolved:{i}"))?,
            space_id.clone(),
            ObstructionType::MissingMorphism,
            explanation,
            Severity::Low,
            make_provenance(&u.file, u.line),
        )
        .with_required_resolution(RequiredResolution::new(format!(
            "resolve reference to {}",
            u.target_text
        ))?);
        if let Some(cell_id) = space_data.fqn_to_cell_id.get(&u.from_fqn) {
            obstruction = obstruction.with_location_cell(Id::new(cell_id)?);
        }
        obstructions.push(obstruction);
    }

    let result = detect_obstruction_completion_candidates(ObstructionCompletionInput::new(
        space_id,
        obstructions,
    ))?;

    let mut stats = CompletionStats::default();
    for candidate in result.candidates() {
        stats.candidates += 1;
        let confidence = candidate.confidence.value();
        if confidence >= high_threshold {
            stats.auto_accepted += 1;
        } else if confidence < low_threshold {
            stats.auto_rejected += 1;
        } else {
            stats.pending += 1;
        }
    }

    Ok(stats)
}

fn make_provenance(file: &str, line: u32) -> Provenance {
    let source_ref = SourceRef::new(SourceKind::Code)
        .with_uri(format!("{file}#L{line}"))
        .expect("valid uri")
        .with_title(file.to_string())
        .expect("valid title");
    Provenance::new(source_ref, Confidence::new(0.5).expect("valid confidence"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use higher_graphen_structure::space::Space;
    use specgraphen_model::space_data::UnresolvedRecord;
    use specgraphen_model::JavaRelationType;

    fn space_with(unresolved: Vec<UnresolvedRecord>) -> SpaceData {
        let space = Space::new(Id::new("test").expect("id"), "test");
        let mut sd = SpaceData::new(space, vec![], vec![], vec![], vec![], Default::default());
        sd.unresolved = unresolved;
        sd
    }

    fn rec() -> UnresolvedRecord {
        UnresolvedRecord {
            from_fqn: "com.example.A.foo".to_string(),
            target_text: "bar".to_string(),
            relation_type: JavaRelationType::Calls,
            file: "A.java".to_string(),
            line: 1,
        }
    }

    #[test]
    fn empty_unresolved_yields_no_candidates() {
        let stats = run_obstruction_completion(&space_with(vec![]), 0.8, 0.4).unwrap();
        assert_eq!(stats.candidates, 0);
    }

    #[test]
    fn fixed_confidence_lands_in_pending_at_default_thresholds() {
        // Unresolved refs carry confidence 0.5; with high=0.8/low=0.4 that is pending.
        let stats = run_obstruction_completion(&space_with(vec![rec(), rec()]), 0.8, 0.4).unwrap();
        assert_eq!(stats.candidates, 2);
        assert_eq!(stats.pending, 2);
        assert_eq!(stats.auto_accepted, 0);
        assert_eq!(stats.auto_rejected, 0);
    }

    #[test]
    fn low_threshold_auto_accepts() {
        let stats = run_obstruction_completion(&space_with(vec![rec()]), 0.4, 0.3).unwrap();
        assert_eq!(stats.auto_accepted, 1);
        assert_eq!(stats.pending, 0);
    }
}
