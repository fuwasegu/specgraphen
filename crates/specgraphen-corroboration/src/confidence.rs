use higher_graphen_core::{Confidence, Id};
use higher_graphen_evidence::confidence::{
    ConfidenceEvidence, ConfidenceUpdateInput, EvidenceLikelihood,
};
use specgraphen_model::SemanticAnnotation;

#[allow(dead_code)]
pub struct CorroborationOutcome {
    pub posterior: f64,
    pub annotation: SemanticAnnotation,
    pub discrepancies: Vec<String>,
}

pub fn compute_corroboration(
    _deterministic_facts: &DeterministicFacts,
    llm_annotation: &LlmAnnotation,
    cell_id: &str,
) -> CorroborationOutcome {
    let mut discrepancies = Vec::new();

    // Prior: tree-sitter structural extraction gives base 0.5
    let prior = Confidence::new(0.5).expect("valid prior");
    let claim_id = Id::new(cell_id).unwrap_or_else(|_| Id::new("unknown").unwrap());

    let mut input = ConfidenceUpdateInput::new(claim_id, prior);

    // Evidence: LLM annotation with witness citations
    let mut supporting = Vec::new();

    if llm_annotation.has_witnesses {
        // If LLM cites specific line numbers, that's strong supporting evidence
        // P(witnesses | claim true) = 0.85, P(witnesses | claim false) = 0.25
        if let Ok(likelihood) = EvidenceLikelihood::new(
            Confidence::new(0.85).unwrap(),
            Confidence::new(0.25).unwrap(),
        ) {
            supporting.push(ConfidenceEvidence {
                evidence_id: Id::new(format!("{cell_id}:witness")).unwrap(),
                summary: "LLM provided witness line citations".to_string(),
                likelihood,
                source_ids: Vec::new(),
            });
        }
    } else {
        discrepancies.push("LLM annotation lacks witness citations".to_string());
    }

    if !llm_annotation.intent.is_empty() {
        // Intent description present: mild supporting evidence
        if let Ok(likelihood) =
            EvidenceLikelihood::new(Confidence::new(0.7).unwrap(), Confidence::new(0.4).unwrap())
        {
            supporting.push(ConfidenceEvidence {
                evidence_id: Id::new(format!("{cell_id}:intent")).unwrap(),
                summary: "LLM provided intent description".to_string(),
                likelihood,
                source_ids: Vec::new(),
            });
        }
    }

    if !llm_annotation.preconditions.is_empty() {
        if let Ok(likelihood) = EvidenceLikelihood::new(
            Confidence::new(0.75).unwrap(),
            Confidence::new(0.35).unwrap(),
        ) {
            supporting.push(ConfidenceEvidence {
                evidence_id: Id::new(format!("{cell_id}:precond")).unwrap(),
                summary: "LLM identified preconditions".to_string(),
                likelihood,
                source_ids: Vec::new(),
            });
        }
    }

    if !llm_annotation.side_effects.is_empty() {
        if let Ok(likelihood) = EvidenceLikelihood::new(
            Confidence::new(0.75).unwrap(),
            Confidence::new(0.35).unwrap(),
        ) {
            supporting.push(ConfidenceEvidence {
                evidence_id: Id::new(format!("{cell_id}:sideeffects")).unwrap(),
                summary: "LLM identified side effects".to_string(),
                likelihood,
                source_ids: Vec::new(),
            });
        }
    }

    input = input.with_supporting_evidence(supporting);

    // Compute posterior via HG Bayesian engine
    let posterior = match higher_graphen_evidence::confidence::update_confidence(input) {
        Ok(record) => record.posterior.value(),
        Err(e) => {
            tracing::warn!("Bayesian update failed: {e}, falling back to prior");
            prior.value()
        }
    };

    let annotation = SemanticAnnotation {
        intent: if !llm_annotation.intent.is_empty() {
            Some(llm_annotation.intent.clone())
        } else {
            None
        },
        behavior: if !llm_annotation.behavior.is_empty() {
            Some(llm_annotation.behavior.clone())
        } else {
            None
        },
        preconditions: llm_annotation.preconditions.clone(),
        postconditions: llm_annotation.postconditions.clone(),
        invariants: Vec::new(),
        side_effects: llm_annotation.side_effects.clone(),
        error_behavior: if !llm_annotation.error_behavior.is_empty() {
            Some(llm_annotation.error_behavior.clone())
        } else {
            None
        },
    };

    CorroborationOutcome {
        posterior,
        annotation,
        discrepancies,
    }
}

#[allow(dead_code)]
pub struct DeterministicFacts {
    pub entity_type: String,
    pub label: String,
    pub has_provenance: bool,
    pub caller_count: usize,
    pub callee_count: usize,
}

pub struct LlmAnnotation {
    pub intent: String,
    pub behavior: String,
    pub preconditions: Vec<String>,
    pub postconditions: Vec<String>,
    pub side_effects: Vec<String>,
    pub error_behavior: String,
    pub has_witnesses: bool,
}

impl LlmAnnotation {
    pub fn from_json(json_str: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
        Some(Self {
            intent: v["intent"].as_str().unwrap_or_default().to_string(),
            behavior: v["behavior"].as_str().unwrap_or_default().to_string(),
            preconditions: v["preconditions"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            postconditions: v["postconditions"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            side_effects: v["side_effects"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            error_behavior: v["error_behavior"].as_str().unwrap_or_default().to_string(),
            has_witnesses: v["witnesses"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false),
        })
    }
}
