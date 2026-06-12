use specgraphen_model::SemanticAnnotation;

#[allow(dead_code)]
pub fn merge_annotations(
    base: &SemanticAnnotation,
    overlay: &SemanticAnnotation,
) -> SemanticAnnotation {
    SemanticAnnotation {
        intent: overlay.intent.clone().or_else(|| base.intent.clone()),
        behavior: overlay.behavior.clone().or_else(|| base.behavior.clone()),
        preconditions: if overlay.preconditions.is_empty() {
            base.preconditions.clone()
        } else {
            overlay.preconditions.clone()
        },
        postconditions: if overlay.postconditions.is_empty() {
            base.postconditions.clone()
        } else {
            overlay.postconditions.clone()
        },
        invariants: if overlay.invariants.is_empty() {
            base.invariants.clone()
        } else {
            overlay.invariants.clone()
        },
        side_effects: if overlay.side_effects.is_empty() {
            base.side_effects.clone()
        } else {
            overlay.side_effects.clone()
        },
        error_behavior: overlay
            .error_behavior
            .clone()
            .or_else(|| base.error_behavior.clone()),
    }
}
