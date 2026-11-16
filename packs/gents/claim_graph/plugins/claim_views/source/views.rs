//! The three views as Markdown, each with the number of rows in its table.
//! Every list is in id order, so the same graph and `generated_at` render the
//! same bytes.
use std::collections::BTreeMap;

use crate::clock;
use crate::graph::{Graph, Status};

pub const VIEWS: [&str; 3] = ["status", "disputes", "reputation"];

/// How much of a target claim's text the disputes table shows.
const TARGET_TEXT_CHARS: usize = 120;

pub struct Rendered {
    pub markdown: String,
    pub rows: usize,
}

pub fn render(view: &str, graph_id: &str, graph: &Graph, generated_at: &str) -> Rendered {
    match view {
        "status" => status(graph_id, graph, generated_at),
        "disputes" => disputes(graph_id, graph, generated_at),
        _ => reputation(graph_id, graph, generated_at),
    }
}

/// A table cell: a pipe would end the cell and a line break the row.
fn cell(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
}

fn row(cells: &[String]) -> String {
    format!("| {} |\n", cells.join(" | "))
}

/// The header and rule of a table; the `numeric` columns align right.
fn header(names: &[&str], numeric: &[usize]) -> String {
    let rule: Vec<String> = (0..names.len())
        .map(|i| if numeric.contains(&i) { "---:" } else { "---" }.to_owned())
        .collect();
    let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    row(&names) + &row(&rule)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn truncated(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_owned();
    }
    let cut: String = text.chars().take(chars).collect();
    format!("{}...", cut.trim_end())
}

fn status(graph_id: &str, graph: &Graph, generated_at: &str) -> Rendered {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut table = String::new();
    let mut rows = 0;
    for id in graph.claims.keys() {
        let effective = graph.effective(id);
        *counts.entry(effective.status.name()).or_default() += 1;
        if let Some(edge) = effective.edge {
            rows += 1;
            table += &row(&[
                cell(id),
                effective.status.name().to_owned(),
                cell(&edge.describe()),
                cell(&edge.by),
            ]);
        }
    }
    let mut md = format!(
        "# Status of {}\n\nGenerated {} from {} and {}.\n\n",
        cell(graph_id),
        cell(generated_at),
        plural(graph.claims.len(), "claim", "claims"),
        plural(graph.edges.len(), "edge", "edges"),
    );
    md += &header(&["Status", "Claims"], &[1]);
    for status in Status::ALL {
        md += &row(&[
            status.name().to_owned(),
            counts.get(status.name()).copied().unwrap_or(0).to_string(),
        ]);
    }
    md += "\n## Claims that are not live\n\n";
    if rows == 0 {
        md += "Every claim is live.\n";
    } else {
        md += &header(&["Claim", "Status", "Deciding edge", "By"], &[]);
        md += &table;
    }
    Rendered { markdown: md, rows }
}

fn disputes(graph_id: &str, graph: &Graph, generated_at: &str) -> Rendered {
    let today = clock::days(generated_at);
    let mut open: Vec<(&str, &str)> = graph
        .edges
        .iter()
        .filter(|e| {
            e.edge_type == "challenges"
                && graph.is_kind(&e.from, "challenge")
                && graph.challenge_open(&e.from)
        })
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    open.sort();
    let mut md = format!(
        "# Open challenges in {}\n\nGenerated {}. {}.\n\n",
        cell(graph_id),
        cell(generated_at),
        plural(open.len(), "open challenge", "open challenges"),
    );
    if open.is_empty() {
        md += "No challenge is open.\n";
        return Rendered { markdown: md, rows: 0 };
    }
    md += &header(
        &[
            "Target",
            "Target text",
            "Challenger",
            "Challenge",
            "Challenge text",
            "Days open",
            "Reviews of the target",
        ],
        &[5],
    );
    for (challenge_id, target_id) in &open {
        let challenge = graph.claims.get(*challenge_id);
        let target_text = graph
            .claims
            .get(*target_id)
            .map_or_else(String::new, |c| truncated(&c.text, TARGET_TEXT_CHARS));
        let days = match (today, challenge.and_then(|c| clock::days(&c.asserted))) {
            (Some(now), Some(asserted)) => (now - asserted).max(0).to_string(),
            _ => "?".to_owned(),
        };
        let reviews: Vec<String> = graph
            .into_claim(target_id, "reviews")
            .filter(|e| graph.is_kind(&e.from, "review"))
            .map(|e| {
                let review = &graph.claims[&e.from];
                format!(
                    "{} {} ({})",
                    review.id,
                    review.verdict.as_deref().unwrap_or("no verdict"),
                    review.by
                )
            })
            .collect();
        md += &row(&[
            cell(target_id),
            cell(&target_text),
            cell(challenge.map_or("", |c| c.by.as_str())),
            cell(challenge_id),
            cell(challenge.map_or("", |c| c.text.as_str())),
            days,
            if reviews.is_empty() {
                "none".to_owned()
            } else {
                cell(&reviews.join("; "))
            },
        ]);
    }
    Rendered {
        markdown: md,
        rows: open.len(),
    }
}

#[derive(Default)]
struct Counts {
    claims: usize,
    sourced: usize,
    memory: usize,
    inferred: usize,
    retracted: usize,
    superseded: usize,
    answered: usize,
    raised: usize,
    closed: usize,
    received: usize,
    verdicts: BTreeMap<String, usize>,
}

fn reputation(graph_id: &str, graph: &Graph, generated_at: &str) -> Rendered {
    let mut table: BTreeMap<&str, Counts> = graph
        .perspectives
        .keys()
        .map(|id| (id.as_str(), Counts::default()))
        .collect();
    for claim in graph.claims.values() {
        let c = table.entry(claim.by.as_str()).or_default();
        c.claims += 1;
        match claim.confidence.as_str() {
            "sourced" => c.sourced += 1,
            "memory" => c.memory += 1,
            "inferred" => c.inferred += 1,
            _ => {}
        }
        let pointed = |edge_type| graph.into_claim(&claim.id, edge_type).next().is_some();
        c.retracted += usize::from(pointed("retracts"));
        c.superseded += usize::from(pointed("supersedes"));
        c.answered += usize::from(pointed("answers"));
        if claim.kind == "challenge" {
            c.raised += 1;
            c.closed += usize::from(!graph.challenge_open(&claim.id));
        }
        if claim.kind == "review" {
            let verdict = claim.verdict.clone().unwrap_or_else(|| "no verdict".into());
            *c.verdicts.entry(verdict).or_default() += 1;
        }
    }
    for edge in &graph.edges {
        if edge.edge_type == "challenges" && graph.is_kind(&edge.from, "challenge") {
            if let Some(target) = graph.claims.get(&edge.to) {
                if let Some(c) = table.get_mut(target.by.as_str()) {
                    c.received += 1;
                }
            }
        }
    }
    table.remove("");
    let mut md = format!(
        "# Reputation in {}\n\nGenerated {}. {}. Counts only, no score: \
         \"Answered\" is the perspective's claims an `answers` edge points at, \
         \"Closed\" its challenges that an `answers` or `resolves` edge closed.\n\n",
        cell(graph_id),
        cell(generated_at),
        plural(table.len(), "perspective", "perspectives"),
    );
    if table.is_empty() {
        md += "The graph has no perspectives.\n";
        return Rendered { markdown: md, rows: 0 };
    }
    md += &header(
        &[
            "Perspective",
            "Name",
            "Claims",
            "Sourced",
            "Memory",
            "Inferred",
            "Retracted",
            "Superseded",
            "Answered",
            "Challenges raised",
            "Closed",
            "Challenges received",
            "Reviews given",
        ],
        &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    );
    for (id, c) in &table {
        let reviews = if c.verdicts.is_empty() {
            "0".to_owned()
        } else {
            let parts: Vec<String> = c.verdicts.iter().map(|(v, n)| format!("{v} {n}")).collect();
            cell(&parts.join(", "))
        };
        let name = graph.perspectives.get(*id).map_or("", String::as_str);
        let mut cells = vec![cell(id), cell(name)];
        cells.extend(
            [
                c.claims, c.sourced, c.memory, c.inferred, c.retracted, c.superseded, c.answered,
                c.raised, c.closed, c.received,
            ]
            .iter()
            .map(usize::to_string),
        );
        cells.push(reviews);
        md += &row(&cells);
    }
    Rendered {
        markdown: md,
        rows: table.len(),
    }
}
