use crate::prelude::*;

#[derive(Debug, Clone)]
pub struct SelectionRangeRequest {
    pub path: PathBuf,
    pub positions: Vec<LspPosition>,
}

pub fn selection_range(
    world: &TypstSystemWorld,
    req: SelectionRangeRequest,
    position_encoding: PositionEncoding,
) -> Option<Vec<SelectionRange>> {
    let source = get_suitable_source_in_workspace(world, &req.path).ok()?;

    let mut ranges = Vec::new();
    for position in req.positions {
        let typst_offset = lsp_to_typst::position_to_offset(position, position_encoding, &source);
        let tree = LinkedNode::new(source.root());
        let leaf = tree.leaf_at(typst_offset)?;
        ranges.push(range_for_node(&source, position_encoding, &leaf));
    }

    Some(ranges)
}

fn range_for_node(
    source: &Source,
    position_encoding: PositionEncoding,
    node: &LinkedNode,
) -> SelectionRange {
    let range = typst_to_lsp::range(node.range(), source, position_encoding);
    SelectionRange {
        range: range.raw_range,
        parent: node
            .parent()
            .map(|node| Box::new(range_for_node(source, position_encoding, node))),
    }
}
