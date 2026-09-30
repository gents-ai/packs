//! Thin helpers over `roxmltree` for the Office, OpenDocument and EPUB readers,
//! which all match elements and attributes by local name.
use roxmltree::{Document, Node, ParsingOptions};

/// Parses XML with DTDs refused, so no document can pull in entities.
pub fn parse(text: &str) -> Result<Document<'_>, String> {
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: 20_000_000,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, options)
        .map_err(|e| format!("the document XML is malformed: {e}"))
}

/// An attribute by local name, ignoring its namespace prefix.
pub fn attr<'a>(node: Node<'a, '_>, local: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == local)
        .map(|a| a.value())
}

pub fn is(node: Node<'_, '_>, local: &str) -> bool {
    node.is_element() && node.tag_name().name() == local
}

pub fn child<'a, 'i>(node: Node<'a, 'i>, local: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| is(*c, local))
}

pub fn descendant<'a, 'i>(node: Node<'a, 'i>, local: &str) -> Option<Node<'a, 'i>> {
    node.descendants().find(|c| is(*c, local))
}

/// All text below a node, without separators.
pub fn text(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}
