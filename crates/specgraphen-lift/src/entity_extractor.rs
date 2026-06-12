use std::collections::HashMap;

use higher_graphen_core::Id;
use higher_graphen_structure::space::Cell;
use specgraphen_model::{CellFactory, DerivationSource, JavaEntityType, WitnessInfo};

pub struct EntityExtractor {
    pub cells: Vec<Cell>,
    pub fqn_to_cell_id: HashMap<String, Id>,
    pub fqn_to_witness: HashMap<String, WitnessInfo>,
    package_name: Option<String>,
    class_stack: Vec<String>,
    file_path: String,
}

impl EntityExtractor {
    pub fn new(file_path: &str) -> Self {
        Self {
            cells: Vec::new(),
            fqn_to_cell_id: HashMap::new(),
            fqn_to_witness: HashMap::new(),
            package_name: None,
            class_stack: Vec::new(),
            file_path: file_path.to_string(),
        }
    }

    pub fn extract(&mut self, node: tree_sitter::Node, source: &[u8], factory: &mut CellFactory) {
        self.visit_node(node, source, factory);
    }

    fn visit_node(&mut self, node: tree_sitter::Node, source: &[u8], factory: &mut CellFactory) {
        match node.kind() {
            "package_declaration" => self.extract_package(node, source, factory),
            "class_declaration" => {
                self.extract_class_like(node, source, factory, JavaEntityType::Class)
            }
            "interface_declaration" => {
                self.extract_class_like(node, source, factory, JavaEntityType::Interface)
            }
            "enum_declaration" => {
                self.extract_class_like(node, source, factory, JavaEntityType::Enum)
            }
            "record_declaration" => {
                self.extract_class_like(node, source, factory, JavaEntityType::Record)
            }
            "annotation_type_declaration" => {
                self.extract_class_like(node, source, factory, JavaEntityType::Annotation)
            }
            "method_declaration" => self.extract_method(node, source, factory),
            "constructor_declaration" => self.extract_constructor(node, source, factory),
            "field_declaration" => self.extract_field(node, source, factory),
            "enum_constant" => self.extract_enum_constant(node, source, factory),
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.visit_node(child, source, factory);
                }
            }
        }
    }

    fn extract_package(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                let name = child.utf8_text(source).unwrap_or_default().to_string();
                let fqn = name.clone();
                let witness = self.make_witness(node);

                if !self.fqn_to_cell_id.contains_key(&fqn) {
                    let cell =
                        factory.create_entity_cell(&JavaEntityType::Package, &fqn, &name, &witness);
                    self.fqn_to_cell_id.insert(fqn.clone(), cell.id.clone());
                    self.fqn_to_witness.insert(fqn, witness);
                    self.cells.push(cell);
                }
                self.package_name = Some(name);
                return;
            }
        }
    }

    fn extract_class_like(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
        entity_type: JavaEntityType,
    ) {
        let name = self.get_field_text(node, "name", source);
        if name.is_empty() {
            return;
        }

        let fqn = self.build_fqn(&name);
        let witness = self.make_witness(node);

        let cell = factory.create_entity_cell(&entity_type, &fqn, &name, &witness);
        self.fqn_to_cell_id.insert(fqn.clone(), cell.id.clone());
        self.fqn_to_witness.insert(fqn, witness);
        self.cells.push(cell);

        self.class_stack.push(name);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "class_body"
                || child.kind() == "interface_body"
                || child.kind() == "enum_body"
                || child.kind() == "record_declaration_body"
                || child.kind() == "annotation_type_body"
            {
                let mut body_cursor = child.walk();
                for body_child in child.children(&mut body_cursor) {
                    self.visit_node(body_child, source, factory);
                }
            }
        }
        self.class_stack.pop();
    }

    fn extract_method(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        let name = self.get_field_text(node, "name", source);
        if name.is_empty() {
            return;
        }

        let params = self.get_parameter_types(node, source);
        let label = format!("{}({})", name, params.join(", "));
        let fqn = self.build_fqn(&name);
        let witness = self.make_witness(node);

        let cell = factory.create_entity_cell(&JavaEntityType::Method, &fqn, &label, &witness);
        self.fqn_to_cell_id.insert(fqn.clone(), cell.id.clone());
        self.fqn_to_witness.insert(fqn, witness);
        self.cells.push(cell);
    }

    fn extract_constructor(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        let name = self.get_field_text(node, "name", source);
        if name.is_empty() {
            return;
        }

        let params = self.get_parameter_types(node, source);
        let label = format!("{}({})", name, params.join(", "));
        let fqn = self.build_fqn(&format!("<init>_{}", params.len()));
        let witness = self.make_witness(node);

        let cell = factory.create_entity_cell(&JavaEntityType::Constructor, &fqn, &label, &witness);
        self.fqn_to_cell_id.insert(fqn.clone(), cell.id.clone());
        self.fqn_to_witness.insert(fqn, witness);
        self.cells.push(cell);
    }

    fn extract_field(&mut self, node: tree_sitter::Node, source: &[u8], factory: &mut CellFactory) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "variable_declarator" {
                let name_node = child.child_by_field_name("name");
                if let Some(n) = name_node {
                    let name = n.utf8_text(source).unwrap_or_default().to_string();
                    let fqn = self.build_fqn(&name);
                    let witness = self.make_witness(node);

                    let cell =
                        factory.create_entity_cell(&JavaEntityType::Field, &fqn, &name, &witness);
                    self.fqn_to_cell_id.insert(fqn.clone(), cell.id.clone());
                    self.fqn_to_witness.insert(fqn, witness);
                    self.cells.push(cell);
                }
            }
        }
    }

    fn extract_enum_constant(
        &mut self,
        node: tree_sitter::Node,
        source: &[u8],
        factory: &mut CellFactory,
    ) {
        let name_node = node.child_by_field_name("name");
        if let Some(n) = name_node {
            let name = n.utf8_text(source).unwrap_or_default().to_string();
            let fqn = self.build_fqn(&name);
            let witness = self.make_witness(node);

            let cell =
                factory.create_entity_cell(&JavaEntityType::EnumConstant, &fqn, &name, &witness);
            self.fqn_to_cell_id.insert(fqn.clone(), cell.id.clone());
            self.fqn_to_witness.insert(fqn, witness);
            self.cells.push(cell);
        }
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

    fn get_field_text(&self, node: tree_sitter::Node, field: &str, source: &[u8]) -> String {
        node.child_by_field_name(field)
            .map(|n| n.utf8_text(source).unwrap_or_default().to_string())
            .unwrap_or_default()
    }

    fn get_parameter_types(&self, node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
        let mut params = Vec::new();
        if let Some(param_list) = node.child_by_field_name("parameters") {
            let mut cursor = param_list.walk();
            for child in param_list.children(&mut cursor) {
                if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
                    if let Some(type_node) = child.child_by_field_name("type") {
                        params.push(type_node.utf8_text(source).unwrap_or_default().to_string());
                    }
                }
            }
        }
        params
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
