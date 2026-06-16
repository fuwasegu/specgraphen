//! Human/audit projection backed by HigherGraphen's projection kernel.
//!
//! DESIGN.md pillar B asks for projections whose information loss is *declared*
//! and checkable. Here we lift the human-facing spec into an HG `Projection`
//! and run `measure_projection_loss` over it, so "what did this spec silently
//! drop" (unannotated members, omitted entities) becomes a machine-checked
//! signal (DESIGN.md §7) rather than an implicit assumption.

use anyhow::Result;
use higher_graphen_core::Id;
use higher_graphen_projection::{
    measure_projection_loss, InformationLoss, OutputSchema, Projection, ProjectionAudience,
    ProjectionOutput, ProjectionPurpose, ProjectionResult, ProjectionSection, ProjectionSelector,
    RendererKind,
};
use serde::{Deserialize, Serialize};
use specgraphen_model::{JavaEntityType, SpaceData};

/// Machine-checked honesty report for the human-facing spec projection.
#[derive(Debug, Serialize, Deserialize)]
pub struct SpecLossReport {
    /// Inferred review severity ("low" / "medium" / "high").
    pub risk_severity: String,
    /// Number of eligible source cells considered (the whole space).
    pub total_sources: usize,
    /// Number of projected sections (one per class-like entity).
    pub projected_sections: usize,
    /// Source cells absent from the projection entirely (e.g. fields, packages).
    pub omitted_count: usize,
    /// Example FQNs the spec omits, for quick drill-down (capped at 5).
    pub omitted_examples: Vec<String>,
    /// Collapsed source pairs (members folded into a class section).
    pub collapsed_pair_count: usize,
    /// Source cells whose loss is measurable but not declared.
    pub missing_loss_declarations: usize,
    /// Structured review signals from the HG loss kernel.
    pub obstructions: Vec<String>,
}

fn to_anyhow(e: higher_graphen_core::CoreError) -> anyhow::Error {
    anyhow::anyhow!("HG projection error: {e}")
}

/// Build the spec projection over class-like entities (members folded in) and
/// measure its information loss with HG's deterministic kernel.
pub fn spec_loss_report(space_data: &SpaceData) -> Result<SpecLossReport> {
    let mut class_entities: Vec<_> = space_data
        .entities
        .iter()
        .filter(|e| {
            matches!(
                e.entity_type,
                JavaEntityType::Class
                    | JavaEntityType::Interface
                    | JavaEntityType::Enum
                    | JavaEntityType::Record
            )
        })
        .collect();
    class_entities.sort_by(|a, b| a.fqn.cmp(&b.fqn));

    if class_entities.is_empty() {
        return Ok(SpecLossReport {
            risk_severity: "low".to_string(),
            total_sources: space_data.cells.len(),
            projected_sections: 0,
            omitted_count: 0,
            omitted_examples: Vec::new(),
            collapsed_pair_count: 0,
            missing_loss_declarations: 0,
            obstructions: Vec::new(),
        });
    }

    let mut section_names = Vec::new();
    let mut sections = Vec::new();
    let mut information_loss = Vec::new();

    for class in &class_entities {
        let prefix = format!("{}.", class.fqn);
        let members: Vec<_> = space_data
            .entities
            .iter()
            .filter(|e| {
                e.fqn.starts_with(&prefix)
                    && matches!(
                        e.entity_type,
                        JavaEntityType::Method | JavaEntityType::Constructor
                    )
            })
            .collect();

        let class_id = Id::new(&class.cell_id).map_err(to_anyhow)?;
        let mut source_ids = vec![class_id];
        let mut member_ids = Vec::new();
        let mut unannotated = Vec::new();
        for m in &members {
            let mid = Id::new(&m.cell_id).map_err(to_anyhow)?;
            source_ids.push(mid.clone());
            let annotated = space_data
                .annotations
                .get(&m.cell_id)
                .map(|a| a.has_content())
                .unwrap_or(false);
            if !annotated {
                unannotated.push(mid.clone());
            }
            member_ids.push(mid);
        }

        // Declare the loss this section incurs: members are folded into the
        // class section, and unannotated members carry only their signature.
        if !member_ids.is_empty() {
            information_loss.push(
                InformationLoss::declared(
                    format!("{} member(s) folded into class section", member_ids.len()),
                    member_ids,
                )
                .map_err(to_anyhow)?,
            );
        }
        if !unannotated.is_empty() {
            information_loss.push(
                InformationLoss::declared(
                    format!(
                        "{} member(s) listed by signature only (not annotated)",
                        unannotated.len()
                    ),
                    unannotated,
                )
                .map_err(to_anyhow)?,
            );
        }

        let body = match space_data
            .annotations
            .get(&class.cell_id)
            .filter(|a| a.has_content())
            .and_then(|a| a.intent.clone())
        {
            Some(intent) => intent,
            None => "signature only".to_string(),
        };

        section_names.push(class.fqn.clone());
        sections
            .push(ProjectionSection::new(class.fqn.clone(), body, source_ids).map_err(to_anyhow)?);
    }

    // The kernel requires at least one declaration even when nothing is lost.
    if information_loss.is_empty() {
        let anchor = Id::new(&class_entities[0].cell_id).map_err(to_anyhow)?;
        information_loss.push(
            InformationLoss::declared("no member-level loss", vec![anchor]).map_err(to_anyhow)?,
        );
    }

    let all_source_ids: Vec<Id> = sections
        .iter()
        .flat_map(|s| s.source_ids().iter().cloned())
        .collect();
    let eligible: Vec<Id> = space_data.cells.iter().map(|c| c.id.clone()).collect();

    let projection = Projection::new(
        Id::new(format!("projection:spec:{}", space_data.space.id.as_str())).map_err(to_anyhow)?,
        space_data.space.id.clone(),
        "spec",
        ProjectionAudience::Human,
        ProjectionPurpose::Report,
        ProjectionSelector::all(),
        OutputSchema::sections(section_names).map_err(to_anyhow)?,
        information_loss.clone(),
    )
    .map_err(to_anyhow)?;

    let output = ProjectionOutput::sections(sections).map_err(to_anyhow)?;
    let result = ProjectionResult::from_projection(
        &projection,
        RendererKind::Markdown,
        output,
        all_source_ids,
        information_loss,
    )
    .map_err(to_anyhow)?;

    let report = measure_projection_loss(&result, &eligible);

    let omitted_examples: Vec<String> = report
        .metric
        .omitted_source_ids
        .iter()
        .take(5)
        .map(|id| {
            space_data
                .entities
                .iter()
                .find(|e| e.cell_id == id.as_str())
                .map(|e| e.fqn.clone())
                .unwrap_or_else(|| id.as_str().to_string())
        })
        .collect();

    Ok(SpecLossReport {
        risk_severity: format!("{:?}", report.ambiguity.risk_severity).to_lowercase(),
        total_sources: report.metric.source_cardinality,
        projected_sections: report.metric.projected_cardinality,
        omitted_count: report.metric.omitted_source_ids.len(),
        omitted_examples,
        collapsed_pair_count: report.metric.collapsed_pair_count,
        missing_loss_declarations: report.ambiguity.missing_loss_declarations.len(),
        obstructions: report
            .ambiguity
            .obstructions
            .iter()
            .map(|o| format!("{o:?}"))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lift_fixture;

    #[test]
    fn reports_sections_and_severity() {
        let sd = lift_fixture();
        let r = spec_loss_report(&sd).unwrap();
        assert!(r.projected_sections > 0, "fixture has class entities");
        assert!(matches!(
            r.risk_severity.as_str(),
            "low" | "medium" | "high"
        ));
        // The spec omits non-class/method entities (fields, packages).
        assert!(r.total_sources >= r.projected_sections);
    }

    #[test]
    fn empty_space_is_low_risk() {
        use higher_graphen_structure::space::Space;
        use specgraphen_model::SpaceData;
        let space = Space::new(higher_graphen_core::Id::new("empty").unwrap(), "empty");
        let sd = SpaceData::new(space, vec![], vec![], vec![], vec![], Default::default());
        let r = spec_loss_report(&sd).unwrap();
        assert_eq!(r.projected_sections, 0);
        assert_eq!(r.risk_severity, "low");
    }
}
