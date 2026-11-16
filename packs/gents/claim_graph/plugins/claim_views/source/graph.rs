//! One graph's claims, edges and perspectives as read from the store, and the
//! effective-status rule over them.
//!
//! For a claim X: `retracted` if any `retracts` edge points at X; else
//! `superseded` if any `supersedes` edge points at X; else `contested` if a
//! `contradicts` edge touches X and no `supersedes` edge (either way) joins
//! the two ends of that edge; else `challenged` if some claim C of kind
//! `challenge` has a `challenges` edge to X and C is open (no `answers` and no
//! `resolves` edge points at C); else `live`. Reviews never change status.
use std::collections::BTreeMap;

use serde_json::{Map, Value};

pub struct Claim {
    pub id: String,
    pub text: String,
    pub kind: String,
    pub by: String,
    pub asserted: String,
    pub confidence: String,
    pub verdict: Option<String>,
}

pub struct Edge {
    pub key: String,
    pub from: String,
    pub to: String,
    pub edge_type: String,
    pub by: String,
}

impl Edge {
    /// `c:a:002 retracts c:a:001`.
    pub fn describe(&self) -> String {
        format!("{} {} {}", self.from, self.edge_type, self.to)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Live,
    Challenged,
    Contested,
    Superseded,
    Retracted,
}

impl Status {
    pub const ALL: [Status; 5] = [
        Status::Live,
        Status::Challenged,
        Status::Contested,
        Status::Superseded,
        Status::Retracted,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Status::Live => "live",
            Status::Challenged => "challenged",
            Status::Contested => "contested",
            Status::Superseded => "superseded",
            Status::Retracted => "retracted",
        }
    }
}

/// A claim's effective status and the edge that decided it (none for live).
pub struct Effective<'a> {
    pub status: Status,
    pub edge: Option<&'a Edge>,
}

pub struct Graph {
    /// By claim id.
    pub claims: BTreeMap<String, Claim>,
    /// In edge-key order.
    pub edges: Vec<Edge>,
    /// Perspective id to name.
    pub perspectives: BTreeMap<String, String>,
    to: BTreeMap<String, Vec<usize>>,
    from: BTreeMap<String, Vec<usize>>,
}

fn text(row: &Map<String, Value>, field: &str) -> String {
    row.get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn id(row: &Value, field: &str, collection: &str) -> Result<String, String> {
    row.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("a {collection} row has no {field}"))
}

fn object<'a>(row: &'a Value, collection: &str) -> Result<&'a Map<String, Value>, String> {
    row.as_object()
        .ok_or_else(|| format!("a {collection} row is not an object"))
}

impl Graph {
    /// The graph from store rows. A row seen twice (the store moved between
    /// pages) counts once.
    pub fn from_rows(
        claims: &[Value],
        edges: &[Value],
        perspectives: &[Value],
    ) -> Result<Graph, String> {
        let mut by_id = BTreeMap::new();
        for row in claims {
            let id = id(row, "claim_id", "Claim")?;
            let o = object(row, "Claim")?;
            let verdict = o.get("verdict").and_then(Value::as_str).map(str::to_owned);
            by_id.insert(
                id.clone(),
                Claim {
                    id,
                    text: text(o, "text"),
                    kind: text(o, "kind"),
                    by: text(o, "by"),
                    asserted: text(o, "asserted"),
                    confidence: text(o, "confidence"),
                    verdict,
                },
            );
        }
        let mut by_key = BTreeMap::new();
        for row in edges {
            let key = id(row, "edge_key", "Edge")?;
            let o = object(row, "Edge")?;
            by_key.insert(
                key.clone(),
                Edge {
                    key,
                    from: text(o, "from_claim"),
                    to: text(o, "to_claim"),
                    edge_type: text(o, "edge_type"),
                    by: text(o, "by"),
                },
            );
        }
        let mut names = BTreeMap::new();
        for row in perspectives {
            let id = id(row, "perspective_id", "Perspective")?;
            names.insert(id, text(object(row, "Perspective")?, "name"));
        }
        Ok(Graph::new(by_id, by_key.into_values().collect(), names))
    }

    pub fn new(
        claims: BTreeMap<String, Claim>,
        mut edges: Vec<Edge>,
        perspectives: BTreeMap<String, String>,
    ) -> Graph {
        edges.sort_by(|a, b| a.key.cmp(&b.key));
        let mut to: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut from: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, edge) in edges.iter().enumerate() {
            to.entry(edge.to.clone()).or_default().push(i);
            from.entry(edge.from.clone()).or_default().push(i);
        }
        Graph {
            claims,
            edges,
            perspectives,
            to,
            from,
        }
    }

    /// Edges of one type pointing at a claim, in edge-key order.
    pub fn into_claim<'a>(&'a self, id: &str, edge_type: &'a str) -> impl Iterator<Item = &'a Edge> {
        self.to
            .get(id)
            .into_iter()
            .flatten()
            .map(|&i| &self.edges[i])
            .filter(move |e| e.edge_type == edge_type)
    }

    /// Edges of one type leaving a claim, in edge-key order.
    pub fn out_of<'a>(&'a self, id: &str, edge_type: &'a str) -> impl Iterator<Item = &'a Edge> {
        self.from
            .get(id)
            .into_iter()
            .flatten()
            .map(|&i| &self.edges[i])
            .filter(move |e| e.edge_type == edge_type)
    }

    fn joined_by_supersedes(&self, a: &str, b: &str) -> bool {
        self.out_of(a, "supersedes").any(|e| e.to == b)
            || self.out_of(b, "supersedes").any(|e| e.to == a)
    }

    /// A challenge is open while no `answers` or `resolves` edge points at it.
    pub fn challenge_open(&self, id: &str) -> bool {
        self.into_claim(id, "answers").next().is_none()
            && self.into_claim(id, "resolves").next().is_none()
    }

    pub fn is_kind(&self, id: &str, kind: &str) -> bool {
        self.claims.get(id).is_some_and(|c| c.kind == kind)
    }

    /// The `challenges` edges to a claim whose challenge is still open.
    pub fn open_challenges_of<'a>(&'a self, id: &str) -> impl Iterator<Item = &'a Edge> {
        self.into_claim(id, "challenges")
            .filter(|e| self.is_kind(&e.from, "challenge") && self.challenge_open(&e.from))
    }

    pub fn effective(&self, id: &str) -> Effective<'_> {
        let decided = |status, edge| Effective {
            status,
            edge: Some(edge),
        };
        if let Some(e) = self.into_claim(id, "retracts").next() {
            return decided(Status::Retracted, e);
        }
        if let Some(e) = self.into_claim(id, "supersedes").next() {
            return decided(Status::Superseded, e);
        }
        let touching = self
            .out_of(id, "contradicts")
            .chain(self.into_claim(id, "contradicts"));
        let mut contested: Vec<&Edge> = touching
            .filter(|e| !self.joined_by_supersedes(&e.from, &e.to))
            .collect();
        contested.sort_by(|a, b| a.key.cmp(&b.key));
        if let Some(e) = contested.first() {
            return decided(Status::Contested, e);
        }
        if let Some(e) = self.open_challenges_of(id).next() {
            return decided(Status::Challenged, e);
        }
        Effective {
            status: Status::Live,
            edge: None,
        }
    }
}
