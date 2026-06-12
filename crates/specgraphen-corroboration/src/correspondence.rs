use higher_graphen_core::{GluingResult, Id, ParticipantRef};
use higher_graphen_reasoning::correspondence::{
    CorrespondenceDetectionInput, CorrespondenceSubject, TypedRelation,
};
use higher_graphen_reasoning::gluing::attempt_gluing;
use specgraphen_model::SpaceData;

#[derive(Debug)]
pub struct CorrespondenceResult {
    pub cell_id: String,
    pub agreement: CorrespondenceAgreement,
    pub overlap_count: usize,
    pub difference_count: usize,
}

#[derive(Debug)]
pub enum CorrespondenceAgreement {
    Agreeing,
    PartialAgreement,
    Conflicting,
    NoCorrespondence,
}

pub fn check_derivation_correspondence(
    _space_data: &SpaceData,
    cell_id: &str,
    treesitter_relations: Vec<TypedRelation>,
    llm_relations: Vec<TypedRelation>,
) -> anyhow::Result<CorrespondenceResult> {
    let cell_hg_id = Id::new(cell_id)?;
    let context_id = Id::new(format!("ctx:corroboration:{cell_id}"))?;
    let provenance_id = Id::new(format!("prov:corroboration:{cell_id}"))?;

    // Build subjects: tree-sitter derivation and LLM derivation
    let ts_subject = CorrespondenceSubject::new(ParticipantRef::Cell(cell_hg_id.clone()))
        .with_role("tree-sitter")?
        .with_normalized_label(format!("ts:{cell_id}"))?
        .with_modality("observed")?
        .with_typed_relations(treesitter_relations);

    let llm_subject = CorrespondenceSubject::new(ParticipantRef::Cell(cell_hg_id.clone()))
        .with_role("llm-behavior")?
        .with_normalized_label(format!("llm:{cell_id}"))?
        .with_modality("inferred")?
        .with_typed_relations(llm_relations);

    let input =
        CorrespondenceDetectionInput::new(context_id, provenance_id, vec![ts_subject, llm_subject]);

    let detection_result =
        higher_graphen_reasoning::correspondence::derive_correspondence_candidates(input)?;

    let candidates = detection_result.into_candidates();

    if candidates.is_empty() {
        return Ok(CorrespondenceResult {
            cell_id: cell_id.to_string(),
            agreement: CorrespondenceAgreement::NoCorrespondence,
            overlap_count: 0,
            difference_count: 0,
        });
    }

    // Attempt gluing on each candidate
    let mut total_overlaps = 0;
    let mut total_differences = 0;
    let mut has_failure = false;
    let mut has_success = false;

    for candidate in &candidates {
        total_overlaps += candidate.overlap_witnesses.len();
        total_differences += candidate.difference_witnesses.len();

        match attempt_gluing(candidate) {
            Ok(attempt) => match attempt.result {
                GluingResult::Success { .. } => has_success = true,
                GluingResult::Candidate { .. } => {}
                GluingResult::Failure { .. } => has_failure = true,
            },
            Err(e) => {
                tracing::trace!("Gluing failed for {cell_id}: {e}");
            }
        }
    }

    let agreement = if has_failure {
        CorrespondenceAgreement::Conflicting
    } else if has_success && total_differences == 0 {
        CorrespondenceAgreement::Agreeing
    } else {
        CorrespondenceAgreement::PartialAgreement
    };

    Ok(CorrespondenceResult {
        cell_id: cell_id.to_string(),
        agreement,
        overlap_count: total_overlaps,
        difference_count: total_differences,
    })
}

pub fn build_typed_relations_from_incidences(
    space_data: &SpaceData,
    cell_id: &str,
) -> Vec<TypedRelation> {
    let mut relations = Vec::new();

    for inc in &space_data.incidences {
        if inc.from_cell_id.as_str() == cell_id {
            let target_fqn = space_data
                .fqn_to_cell_id
                .iter()
                .find(|(_, v)| v.as_str() == inc.to_cell_id.as_str())
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| inc.to_cell_id.as_str().to_string());

            relations.push(TypedRelation {
                subject: cell_id.to_string(),
                relation: inc.relation_type.clone(),
                object: target_fqn,
            });
        }

        if inc.to_cell_id.as_str() == cell_id {
            let source_fqn = space_data
                .fqn_to_cell_id
                .iter()
                .find(|(_, v)| v.as_str() == inc.from_cell_id.as_str())
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| inc.from_cell_id.as_str().to_string());

            relations.push(TypedRelation {
                subject: source_fqn,
                relation: inc.relation_type.clone(),
                object: cell_id.to_string(),
            });
        }
    }

    relations
}
