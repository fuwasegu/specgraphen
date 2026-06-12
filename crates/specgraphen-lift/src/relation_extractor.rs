use std::collections::HashMap;

use higher_graphen_core::Id;
use higher_graphen_structure::space::Incidence;
use specgraphen_model::{CellFactory, DerivationSource, JavaRelationType, WitnessInfo};

pub struct RelationExtractor {
    pub incidences: Vec<Incidence>,
    pub unresolved: Vec<UnresolvedRelation>,
    fqn_to_cell_id: HashMap<String, Id>,
    resolved_cache: HashMap<String, String>,
    package_name: Option<String>,
    class_stack: Vec<String>,
    current_method_fqn: Option<String>,
    file_path: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct UnresolvedRelation {
    pub from_fqn: String,
    pub target_text: String,
    pub relation_type: JavaRelationType,
    pub witness: WitnessInfo,
}

impl RelationExtractor {
    pub fn new(fqn_to_cell_id: HashMap<String, Id>, file_path: &str) -> Self {
        Self {
            incidences: Vec::new(),
            unresolved: Vec::new(),
            fqn_to_cell_id,
            resolved_cache: HashMap::new(),
            package_name: None,
            class_stack: Vec::new(),
            current_method_fqn: None,
            file_path: file_path.to_string(),
        }
    }

    pub fn with_resolved_cache(mut self, cache: HashMap<String, String>) -> Self {
        self.resolved_cache = cache;
        self
    }

    pub fn extract(&mut self, node: tree_sitter::Node, source: &[u8], factory: &mut CellFactory) {
        self.visit_node(node, source, factory);
    }

    fn visit_node(&mut self, node: tree_sitter::Node, source: &[u8], factory: &mut CellFactory) {
        match node.kind() {
            "package_declaration" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                        self.package_name =
                            Some(child.utf8_text(source).unwrap_or_default().to_string());
                    }
                }
            }
            "import_declaration" => self.extract_import(node, source, factory),
            "class_declaration" => {
                self.extract_class_relations(node, source, factory, "class_body")
            }
            "interface_declaration" => self.extract_interface_relations(node, source, factory),
            "enum_declaration" => self.extract_class_relations(node, source, factory, "enum_body"),
            "record_declaration" => {
                self.extract_class_relations(node, source, factory, "record_declaration_body")
            }
            "method_declaration" | "constructor_declaration" => {
                self.extract_method_relations(node, source, factory)
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.visit_node(child, source, factory);
                }
            }
        }
    }

    fn extract_import(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        let text = node.utf8_text(source).unwrap_or_default();
        let import_target = text
            .trim()
            .trim_start_matches("import ")
            .trim_start_matches("static ")
            .trim_end_matches(';')
            .trim()
            .to_string();

        if import_target.ends_with(".*") {
            return;
        }

        let current_class_fqn = self.current_class_fqn();
        if let Some(from_id) =
            current_class_fqn.and_then(|fqn| self.fqn_to_cell_id.get(&fqn).cloned())
        {
            if let Some(to_id) = self.fqn_to_cell_id.get(&import_target).cloned() {
                let witness = self.make_witness(node);
                let inc = factory.create_relation_incidence(
                    &JavaRelationType::Imports,
                    from_id,
                    to_id,
                    &witness,
                );
                self.incidences.push(inc);
            }
        }
    }

    fn extract_class_relations(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
        body_kind: &str,
    ) {
        let name = node
            .child_by_field_name("name")
            .map(|n| n.utf8_text(source).unwrap_or_default().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            return;
        }

        let class_fqn = self.build_fqn(&name);

        // ContainedIn: class → package or enclosing class
        if let Some(from_id) = self.fqn_to_cell_id.get(&class_fqn).cloned() {
            let container_fqn = if self.class_stack.is_empty() {
                self.package_name.clone()
            } else {
                Some(self.current_class_fqn_inner())
            };
            if let Some(ref cfqn) = container_fqn {
                if let Some(to_id) = self.fqn_to_cell_id.get(cfqn).cloned() {
                    let witness = self.make_witness(node);
                    let inc = factory.create_relation_incidence(
                        &JavaRelationType::ContainedIn,
                        from_id.clone(),
                        to_id,
                        &witness,
                    );
                    self.incidences.push(inc);
                }
            }

            // Extends
            if let Some(superclass_node) = node.child_by_field_name("superclass") {
                self.extract_type_ref_relation(
                    superclass_node,
                    source,
                    factory,
                    from_id.clone(),
                    JavaRelationType::Extends,
                );
            }

            // Implements
            if let Some(ifaces_node) = node.child_by_field_name("interfaces") {
                self.extract_type_list_relations(
                    ifaces_node,
                    source,
                    factory,
                    from_id.clone(),
                    JavaRelationType::Implements,
                );
            }

            // Annotations
            self.extract_annotations(node, source, factory, from_id);
        }

        self.class_stack.push(name);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == body_kind {
                let mut body_cursor = child.walk();
                for body_child in child.children(&mut body_cursor) {
                    self.visit_node(body_child, source, factory);
                }
            }
        }
        self.class_stack.pop();
    }

    fn extract_interface_relations(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        let name = node
            .child_by_field_name("name")
            .map(|n| n.utf8_text(source).unwrap_or_default().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            return;
        }

        let iface_fqn = self.build_fqn(&name);
        if let Some(from_id) = self.fqn_to_cell_id.get(&iface_fqn).cloned() {
            let container_fqn = if self.class_stack.is_empty() {
                self.package_name.clone()
            } else {
                Some(self.current_class_fqn_inner())
            };
            if let Some(ref cfqn) = container_fqn {
                if let Some(to_id) = self.fqn_to_cell_id.get(cfqn).cloned() {
                    let witness = self.make_witness(node);
                    let inc = factory.create_relation_incidence(
                        &JavaRelationType::ContainedIn,
                        from_id.clone(),
                        to_id,
                        &witness,
                    );
                    self.incidences.push(inc);
                }
            }

            // extends_interfaces
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "extends_interfaces" {
                    self.extract_type_list_relations(
                        child,
                        source,
                        factory,
                        from_id.clone(),
                        JavaRelationType::Extends,
                    );
                }
            }

            self.extract_annotations(node, source, factory, from_id);
        }

        self.class_stack.push(name);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "interface_body" {
                let mut body_cursor = child.walk();
                for body_child in child.children(&mut body_cursor) {
                    self.visit_node(body_child, source, factory);
                }
            }
        }
        self.class_stack.pop();
    }

    fn extract_method_relations(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        let name = node
            .child_by_field_name("name")
            .map(|n| n.utf8_text(source).unwrap_or_default().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            return;
        }

        let method_fqn = if node.kind() == "constructor_declaration" {
            let params = self.get_parameter_count(node, source);
            self.build_fqn(&format!("<init>_{params}"))
        } else {
            self.build_fqn(&name)
        };

        let prev_method = self.current_method_fqn.take();
        self.current_method_fqn = Some(method_fqn.clone());

        if let Some(from_id) = self.fqn_to_cell_id.get(&method_fqn).cloned() {
            // ContainedIn → enclosing class
            let class_fqn = self.current_class_fqn_inner();
            if let Some(to_id) = self.fqn_to_cell_id.get(&class_fqn).cloned() {
                let witness = self.make_witness(node);
                let inc = factory.create_relation_incidence(
                    &JavaRelationType::ContainedIn,
                    from_id.clone(),
                    to_id,
                    &witness,
                );
                self.incidences.push(inc);
            }

            // Throws
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "throws" {
                    self.extract_type_list_relations(
                        child,
                        source,
                        factory,
                        from_id.clone(),
                        JavaRelationType::Throws,
                    );
                }
            }

            self.extract_annotations(node, source, factory, from_id);
        }

        // Walk body for calls, constructions, field accesses
        if let Some(body) = node.child_by_field_name("body") {
            self.extract_body_relations(body, source, factory);
        }

        self.current_method_fqn = prev_method;
    }

    fn extract_body_relations(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        match node.kind() {
            "method_invocation" => {
                self.extract_call(node, source, factory);
            }
            "object_creation_expression" => {
                self.extract_construction(node, source, factory);
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.extract_body_relations(child, source, factory);
        }
    }

    fn extract_call(&mut self, node: tree_sitter::Node, source: &[u8], factory: &mut CellFactory) {
        let method_name = node
            .child_by_field_name("name")
            .map(|n| n.utf8_text(source).unwrap_or_default().to_string())
            .unwrap_or_default();
        if method_name.is_empty() {
            return;
        }

        let caller_fqn = match &self.current_method_fqn {
            Some(fqn) => fqn.clone(),
            None => return,
        };

        let from_id = match self.fqn_to_cell_id.get(&caller_fqn) {
            Some(id) => id.clone(),
            None => return,
        };

        let object_text = node
            .child_by_field_name("object")
            .map(|n| n.utf8_text(source).unwrap_or_default().to_string());

        let (candidate_fqns, derivation) =
            self.resolve_call_target(&method_name, object_text.as_deref());
        let mut witness = self.make_witness(node);
        witness.derivation_source = derivation;

        let mut resolved = false;
        for fqn in &candidate_fqns {
            if let Some(to_id) = self.fqn_to_cell_id.get(fqn).cloned() {
                let inc = factory.create_relation_incidence(
                    &JavaRelationType::Calls,
                    from_id.clone(),
                    to_id,
                    &witness,
                );
                self.incidences.push(inc);
                resolved = true;
                break;
            }
        }

        if !resolved {
            self.unresolved.push(UnresolvedRelation {
                from_fqn: caller_fqn,
                target_text: if let Some(obj) = &object_text {
                    format!("{obj}.{method_name}")
                } else {
                    method_name
                },
                relation_type: JavaRelationType::Calls,
                witness,
            });
        }
    }

    fn extract_construction(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        let type_name = node
            .child_by_field_name("type")
            .map(|n| n.utf8_text(source).unwrap_or_default().to_string())
            .unwrap_or_default();
        if type_name.is_empty() {
            return;
        }

        let caller_fqn = match &self.current_method_fqn {
            Some(fqn) => fqn.clone(),
            None => return,
        };
        let from_id = match self.fqn_to_cell_id.get(&caller_fqn) {
            Some(id) => id.clone(),
            None => return,
        };

        let (candidate_fqns, derivation) = self.resolve_type_name(&type_name);
        let mut witness = self.make_witness(node);
        witness.derivation_source = derivation;

        let mut resolved = false;
        for fqn in &candidate_fqns {
            if let Some(to_id) = self.fqn_to_cell_id.get(fqn).cloned() {
                let inc = factory.create_relation_incidence(
                    &JavaRelationType::Constructs,
                    from_id.clone(),
                    to_id,
                    &witness,
                );
                self.incidences.push(inc);
                resolved = true;
                break;
            }
        }

        if !resolved {
            self.unresolved.push(UnresolvedRelation {
                from_fqn: caller_fqn,
                target_text: format!("new {type_name}"),
                relation_type: JavaRelationType::Constructs,
                witness,
            });
        }
    }

    fn extract_type_ref_relation(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
        from_id: Id,
        rel_type: JavaRelationType,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "type_identifier" || child.kind() == "scoped_type_identifier" {
                let type_name = child.utf8_text(source).unwrap_or_default().to_string();
                let (candidate_fqns, derivation) = self.resolve_type_name(&type_name);
                let mut witness = self.make_witness(child);
                witness.derivation_source = derivation;

                for fqn in &candidate_fqns {
                    if let Some(to_id) = self.fqn_to_cell_id.get(fqn).cloned() {
                        let inc = factory.create_relation_incidence(
                            &rel_type,
                            from_id.clone(),
                            to_id,
                            &witness,
                        );
                        self.incidences.push(inc);
                        return;
                    }
                }
            }
        }
    }

    fn extract_type_list_relations(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
        from_id: Id,
        rel_type: JavaRelationType,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "type_identifier" || child.kind() == "scoped_type_identifier" {
                let type_name = child.utf8_text(source).unwrap_or_default().to_string();
                let (candidate_fqns, derivation) = self.resolve_type_name(&type_name);
                let mut witness = self.make_witness(child);
                witness.derivation_source = derivation;

                for fqn in &candidate_fqns {
                    if let Some(to_id) = self.fqn_to_cell_id.get(fqn).cloned() {
                        let inc = factory.create_relation_incidence(
                            &rel_type,
                            from_id.clone(),
                            to_id,
                            &witness,
                        );
                        self.incidences.push(inc);
                        break;
                    }
                }
            } else if child.kind() == "type_list" {
                self.extract_type_list_relations(
                    child,
                    source,
                    factory,
                    from_id.clone(),
                    rel_type.clone(),
                );
            }
        }
    }

    fn extract_annotations(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
        from_id: Id,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "marker_annotation" || child.kind() == "annotation" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let ann_name = name_node.utf8_text(source).unwrap_or_default().to_string();
                    let (candidate_fqns, derivation) = self.resolve_type_name(&ann_name);
                    let mut witness = self.make_witness(child);
                    witness.derivation_source = derivation;

                    for fqn in &candidate_fqns {
                        if let Some(to_id) = self.fqn_to_cell_id.get(fqn).cloned() {
                            let inc = factory.create_relation_incidence(
                                &JavaRelationType::AnnotatedWith,
                                from_id.clone(),
                                to_id,
                                &witness,
                            );
                            self.incidences.push(inc);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Returns candidate FQNs and the source that produced them
    /// (`Lsp` on a resolver-cache hit, `TreeSitter` for heuristic candidates).
    fn resolve_call_target(
        &self,
        method_name: &str,
        object: Option<&str>,
    ) -> (Vec<String>, DerivationSource) {
        // Check LSP resolver cache first
        let cache_key = if let Some(obj) = object {
            format!("{obj}.{method_name}")
        } else {
            method_name.to_string()
        };
        if let Some(resolved_fqn) = self.resolved_cache.get(&cache_key) {
            return (vec![resolved_fqn.clone()], DerivationSource::Lsp);
        }

        let mut candidates = Vec::new();

        if let Some(obj) = object {
            if let Some(pkg) = &self.package_name {
                candidates.push(format!("{pkg}.{obj}.{method_name}"));
            }
            candidates.push(format!("{obj}.{method_name}"));
        } else {
            let class_fqn = self.current_class_fqn_inner();
            candidates.push(format!("{class_fqn}.{method_name}"));
        }

        (candidates, DerivationSource::TreeSitter)
    }

    fn resolve_type_name(&self, type_name: &str) -> (Vec<String>, DerivationSource) {
        // Check LSP resolver cache first
        if let Some(resolved_fqn) = self.resolved_cache.get(type_name) {
            return (vec![resolved_fqn.clone()], DerivationSource::Lsp);
        }

        let mut candidates = Vec::new();

        if type_name.contains('.') {
            candidates.push(type_name.to_string());
        }

        if let Some(pkg) = &self.package_name {
            candidates.push(format!("{pkg}.{type_name}"));
        }

        // Check all known FQNs that end with this type name
        for fqn in self.fqn_to_cell_id.keys() {
            if (fqn.ends_with(&format!(".{type_name}")) || fqn == type_name)
                && !candidates.contains(fqn)
            {
                candidates.push(fqn.clone());
            }
        }

        (candidates, DerivationSource::TreeSitter)
    }

    fn build_fqn(&self, name: &str) -> String {
        let mut parts = Vec::new();
        if let Some(ref pkg) = self.package_name {
            parts.push(pkg.clone());
        }
        for cls in &self.class_stack {
            parts.push(cls.clone());
        }
        parts.push(name.to_string());
        parts.join(".")
    }

    fn current_class_fqn(&self) -> Option<String> {
        if self.class_stack.is_empty() {
            None
        } else {
            Some(self.current_class_fqn_inner())
        }
    }

    fn current_class_fqn_inner(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ref pkg) = self.package_name {
            parts.push(pkg.clone());
        }
        for cls in &self.class_stack {
            parts.push(cls.clone());
        }
        parts.join(".")
    }

    fn get_parameter_count(&self, node: tree_sitter::Node, _source: &[u8]) -> usize {
        node.child_by_field_name("parameters")
            .map(|p| {
                let mut count = 0;
                let mut cursor = p.walk();
                for child in p.children(&mut cursor) {
                    if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
                        count += 1;
                    }
                }
                count
            })
            .unwrap_or(0)
    }

    fn make_witness(&self, node: tree_sitter::Node) -> WitnessInfo {
        WitnessInfo {
            file: self.file_path.clone(),
            start_line: node.start_position().row as u32 + 1,
            end_line: node.end_position().row as u32 + 1,
            start_col: node.start_position().column as u32,
            end_col: node.end_position().column as u32,
            derivation_source: DerivationSource::TreeSitter,
        }
    }
}
