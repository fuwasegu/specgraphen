use specgraphen_model::SpaceData;

pub fn resolve_symbol<'a>(
    space_data: &'a SpaceData,
    symbol: &'a str,
) -> Option<(&'a str, &'a str)> {
    // Exact FQN match
    if let Some(cell_id) = space_data.fqn_to_cell_id.get(symbol) {
        return Some((symbol, cell_id.as_str()));
    }

    // Suffix match: find FQNs ending with the symbol
    let mut matches: Vec<_> = space_data
        .fqn_to_cell_id
        .iter()
        .filter(|(fqn, _)| fqn.ends_with(&format!(".{symbol}")) || fqn.as_str() == symbol)
        .collect();

    if matches.len() == 1 {
        let (fqn, cell_id) = matches[0];
        return Some((fqn.as_str(), cell_id.as_str()));
    }

    // If multiple matches, prefer the shortest FQN (most specific)
    if !matches.is_empty() {
        matches.sort_by_key(|(fqn, _)| fqn.len());
        let (fqn, cell_id) = matches[0];
        return Some((fqn.as_str(), cell_id.as_str()));
    }

    None
}
