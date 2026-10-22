//! The bundled templates render with their `{{key}}` placeholders replaced (L2 section 4.7).
//! The tests assert structure and behaviour, never wording (U15).
use rhai::{Dynamic, Map};
use workflow::helpers::render_template;

const NAMES: [&str; 3] = ["report", "research_brief", "review_findings"];

/// The `{{key}}` names left in `text`, in order.
fn keys(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after.find("}}").expect("an unclosed placeholder");
        found.push(after[..end].to_string());
        rest = &after[end + 2..];
    }
    found
}

#[test]
fn each_bundled_template_renders_its_keys() {
    for name in NAMES {
        let raw = render_template(name, &Map::new()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut wanted = keys(&raw);
        wanted.sort();
        wanted.dedup();
        assert!(!wanted.is_empty(), "{name} has no placeholder");
        let map: Map = (wanted.iter())
            .map(|k| (k.as_str().into(), Dynamic::from(format!("value-of-{k}"))))
            .collect();
        let filled = render_template(name, &map).unwrap();
        assert!(
            keys(&filled).is_empty(),
            "{name} kept a placeholder: {filled}"
        );
        for k in &wanted {
            assert!(filled.contains(&format!("value-of-{k}")), "{name}: {k}");
        }
    }
}
