//! The store lookups a batch needs, asked through the host's `data_calls`
//! rounds. The plan is recomputed from the batch every round (the host
//! resends the same input), so `state` carries only the cursor, the requests
//! in flight, follow-up pages and what the store answered. Every lookup is
//! scoped to the batch's `graph_id`: ids are unique per graph, so two graphs
//! may share them. A stored source's text is matched against the quotes
//! citing it in the round it arrives; `state` keeps only the matches.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::batch::Batch;
use crate::profile;
use crate::quotes::{self, Origin};
use crate::rules::{edge_key, ids};

/// Ids per `_in` query, and rows per query: the host's limit.
pub const CHUNK: usize = 256;
/// Queries per round: the host's limit.
pub const PER_ROUND: usize = 16;
/// Queries a batch may need: the host serves 64 rounds, and a full chunk of
/// a historical-edge lookup may need follow-up pages.
pub const MAX_QUERIES: usize = 60 * PER_ROUND;
/// Ids per text query of `Source`: one. A text is at most 200 000
/// characters (about 1.2 MB as JSON at worst) and the host drops rows past
/// the plugin's output budget (4 MiB) without saying so, so a chunk of
/// several texts could lose a row that would then read as a missing source.
pub const TEXT_CHUNK: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The graph's latest ClaimGraph version.
    Profile,
    Claim,
    Entity,
    Perspective,
    Edge,
    /// Edges of type supersedes or retracts pointing at a batch claim.
    Historical,
    /// The status of stored sources the batch cites without needing their
    /// text (in `sources[]`, or the `from` of a self-quoting claim).
    Source,
    /// The status and text of a stored source a claim's `from` cites.
    SourceText,
}

impl Lookup {
    pub fn collection(self) -> &'static str {
        match self {
            Lookup::Profile => "ClaimGraph",
            Lookup::Claim => "Claim",
            Lookup::Entity => "Entity",
            Lookup::Perspective => "Perspective",
            Lookup::Edge | Lookup::Historical => "Edge",
            Lookup::Source | Lookup::SourceText => "Source",
        }
    }

    pub fn field(self) -> &'static str {
        match self {
            Lookup::Profile => "version",
            Lookup::Claim => "claim_id",
            Lookup::Entity => "entity_id",
            Lookup::Perspective => "perspective_id",
            Lookup::Edge => "edge_key",
            Lookup::Historical => "to_claim",
            Lookup::Source | Lookup::SourceText => "source_id",
        }
    }

    fn fields(self) -> Vec<&'static str> {
        match self {
            Lookup::Profile => profile::stored_fields(),
            // A claim's kind decides whether a challenges or reviews edge
            // from it is well formed.
            Lookup::Claim => vec!["claim_id", "kind"],
            Lookup::Source => vec!["source_id", "status"],
            Lookup::SourceText => vec!["source_id", "status", "text"],
            other => vec![other.field()],
        }
    }

    fn filter(self, graph_id: &str, ids: &[String]) -> Value {
        let graph = json!({"graph_id": {"_eq": graph_id}});
        if self == Lookup::Profile {
            return graph;
        }
        let mut by_id = Map::new();
        by_id.insert(self.field().to_owned(), json!({"_in": ids}));
        let mut clauses = vec![graph, Value::Object(by_id)];
        if self == Lookup::Historical {
            clauses.push(json!({"edge_type": {"_in": ["supersedes", "retracts"]}}));
        }
        json!({"_and": clauses})
    }
}

pub struct Query {
    pub lookup: Lookup,
    pub ids: Vec<String>,
}

/// What the store holds of the ids asked about.
#[derive(Default, Debug)]
pub struct Found {
    pub claims: BTreeSet<String>,
    /// The kind of each stored claim found.
    pub claim_kinds: BTreeMap<String, String>,
    pub entities: BTreeSet<String>,
    pub perspectives: BTreeSet<String>,
    pub edges: BTreeSet<String>,
    pub historical: BTreeSet<String>,
    /// The graph's latest stored ClaimGraph row, when there is one.
    pub profile: Option<Value>,
    /// The status of each stored source found.
    pub sources: BTreeMap<String, String>,
    /// Batch claims whose quote occurs in the text of the source their
    /// `from` cites. A text is matched as it arrives and never kept.
    pub source_quotes: BTreeSet<String>,
}

impl Found {
    fn set(&mut self, lookup: Lookup) -> Option<&mut BTreeSet<String>> {
        match lookup {
            Lookup::Profile | Lookup::Source | Lookup::SourceText => None,
            Lookup::Claim => Some(&mut self.claims),
            Lookup::Entity => Some(&mut self.entities),
            Lookup::Perspective => Some(&mut self.perspectives),
            Lookup::Edge => Some(&mut self.edges),
            Lookup::Historical => Some(&mut self.historical),
        }
    }

    /// Whether the graph holds the source with status `ok`.
    pub fn source_ok(&self, id: &str) -> bool {
        self.sources.get(id).is_some_and(|status| status == "ok")
    }

    /// Records a stored source's status and, for an `ok` one whose text came
    /// with it, which batch claims citing it in `from` quote it verbatim.
    fn absorb_source(&mut self, batch: &Batch, row: &Value) {
        let Some(id) = row.get("source_id").and_then(Value::as_str) else {
            return;
        };
        let status = row.get("status").and_then(Value::as_str).unwrap_or("");
        self.sources.insert(id.to_owned(), status.to_owned());
        let Some(text) = row.get("text").and_then(Value::as_str).filter(|_| status == "ok") else {
            return;
        };
        for claim in &batch.claims {
            if quotes::origin(claim) != Some(Origin::Source(id)) {
                continue;
            }
            if let (Some(claim_id), Some(quote)) = (claim.str("id"), claim.str("quote")) {
                if quotes::grep_fixed(text.as_bytes(), quote) {
                    self.source_quotes.insert(claim_id.to_owned());
                }
            }
        }
    }

    /// The highest stored version of the graph's profile, 0 when none.
    pub fn latest_version(&self) -> i64 {
        self.profile
            .as_ref()
            .and_then(|row| row.get("version"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    }
}

const FOUND_KEYS: [(&str, Lookup); 5] = [
    ("claims", Lookup::Claim),
    ("entities", Lookup::Entity),
    ("perspectives", Lookup::Perspective),
    ("edges", Lookup::Edge),
    ("historical", Lookup::Historical),
];

/// The profile lookup, then one query per chunk of ids, per lookup. Each id
/// set is the batch's own ids (whose presence in the store is a duplicate)
/// plus the ids the batch references but does not carry (whose absence is
/// an unknown reference). `resolvers` are an inline profile's
/// `resolver_perspectives`, which must be known perspectives.
pub fn plan(batch: &Batch, resolvers: &[String]) -> Vec<Query> {
    let claim_ids = ids(&batch.claims);
    let entity_ids = ids(&batch.entities);
    let perspective_ids = ids(&batch.perspectives);

    let mut claims = claim_ids.clone();
    for edge in &batch.edges {
        claims.extend(["from", "to"].iter().filter_map(|k| edge.str(k)).map(str::to_owned));
    }
    let mut entities = entity_ids.clone();
    let mut perspectives = perspective_ids.clone();
    perspectives.extend(resolvers.iter().cloned());
    for claim in &batch.claims {
        for entity in claim
            .fields
            .get("entities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            entities.insert(entity.to_owned());
        }
    }
    for record in batch.claims.iter().chain(&batch.edges) {
        if let Some(by) = record.str("by") {
            perspectives.insert(by.to_owned());
        }
    }
    let edges: BTreeSet<String> = batch.edges.iter().filter_map(edge_key).collect();
    let mut sources = BTreeSet::new();
    let mut texts = BTreeSet::new();
    for claim in &batch.claims {
        sources.extend(quotes::cited_sources(claim).into_iter().map(str::to_owned));
        if let Some(Origin::Source(id)) = quotes::origin(claim) {
            if !id.is_empty() && !quotes::self_quoted(claim) {
                texts.insert(id.to_owned());
            }
        }
    }
    let statuses: BTreeSet<String> = sources.difference(&texts).cloned().collect();
    let historical = if batch.source_root.is_some() || !texts.is_empty() {
        claim_ids
    } else {
        BTreeSet::new()
    };

    let mut queries = vec![Query {
        lookup: Lookup::Profile,
        ids: Vec::new(),
    }];
    for (lookup, set) in [
        (Lookup::Claim, claims),
        (Lookup::Entity, entities),
        (Lookup::Perspective, perspectives),
        (Lookup::Edge, edges),
        (Lookup::Historical, historical),
        (Lookup::Source, statuses),
        (Lookup::SourceText, texts),
    ] {
        let all: Vec<String> = set.into_iter().collect();
        let size = if lookup == Lookup::SourceText {
            TEXT_CHUNK
        } else {
            CHUNK
        };
        for chunk in all.chunks(size) {
            queries.push(Query {
                lookup,
                ids: chunk.to_vec(),
            });
        }
    }
    queries
}

/// Progress across rounds.
#[derive(Default, Debug)]
pub struct State {
    /// The next plan query not yet asked.
    pub cursor: usize,
    /// Follow-up pages `(plan index, offset)` of a query whose answer was full.
    pub pages: Vec<(usize, u32)>,
    /// Request ids asked in the previous round.
    pub asked: Vec<String>,
    pub found: Found,
}

fn request_id(index: usize, offset: u32) -> String {
    if offset == 0 {
        format!("q{index}")
    } else {
        format!("q{index}-{offset}")
    }
}

fn parse_request_id(id: &str) -> Option<(usize, u32)> {
    let rest = id.strip_prefix('q')?;
    match rest.split_once('-') {
        Some((index, offset)) => Some((index.parse().ok()?, offset.parse().ok()?)),
        None => Some((rest.parse().ok()?, 0)),
    }
}

impl State {
    pub fn to_json(&self) -> Value {
        let mut found = Map::new();
        let mut copy = |key: &str, set: &BTreeSet<String>| {
            found.insert(key.to_owned(), json!(set));
        };
        copy("claims", &self.found.claims);
        copy("entities", &self.found.entities);
        copy("perspectives", &self.found.perspectives);
        copy("edges", &self.found.edges);
        copy("historical", &self.found.historical);
        found.insert("claim_kinds".to_owned(), json!(self.found.claim_kinds));
        found.insert("sources".to_owned(), json!(self.found.sources));
        found.insert("source_quotes".to_owned(), json!(self.found.source_quotes));
        found.insert(
            "profile".to_owned(),
            self.found.profile.clone().unwrap_or(Value::Null),
        );
        json!({
            "cursor": self.cursor,
            "pages": self.pages,
            "asked": self.asked,
            "found": found,
        })
    }

    /// The state this plugin handed the host last round.
    pub fn from_json(value: Option<&Value>) -> Result<State, String> {
        let bad = || "state is not the state this plugin returned".to_owned();
        let state = value.and_then(Value::as_object).ok_or_else(bad)?;
        let strings = |value: Option<&Value>| -> Result<Vec<String>, String> {
            value
                .and_then(Value::as_array)
                .ok_or_else(bad)?
                .iter()
                .map(|v| v.as_str().map(str::to_owned).ok_or_else(bad))
                .collect()
        };
        let mut found = Found::default();
        let found_json = state.get("found").and_then(Value::as_object).ok_or_else(bad)?;
        for (key, lookup) in FOUND_KEYS {
            if let Some(set) = found.set(lookup) {
                set.extend(strings(found_json.get(key))?);
            }
        }
        for (id, kind) in found_json
            .get("claim_kinds")
            .and_then(Value::as_object)
            .ok_or_else(bad)?
        {
            let kind = kind.as_str().ok_or_else(bad)?.to_owned();
            found.claim_kinds.insert(id.clone(), kind);
        }
        for (id, status) in found_json
            .get("sources")
            .and_then(Value::as_object)
            .ok_or_else(bad)?
        {
            let status = status.as_str().ok_or_else(bad)?.to_owned();
            found.sources.insert(id.clone(), status);
        }
        found
            .source_quotes
            .extend(strings(found_json.get("source_quotes"))?);
        found.profile = found_json.get("profile").filter(|p| !p.is_null()).cloned();
        let pages = state
            .get("pages")
            .and_then(Value::as_array)
            .ok_or_else(bad)?
            .iter()
            .map(|page| {
                let index = page.get(0).and_then(Value::as_u64).ok_or_else(bad)?;
                let offset = page.get(1).and_then(Value::as_u64).ok_or_else(bad)?;
                Ok((index as usize, u32::try_from(offset).map_err(|_| bad())?))
            })
            .collect::<Result<_, String>>()?;
        Ok(State {
            cursor: state.get("cursor").and_then(Value::as_u64).ok_or_else(bad)? as usize,
            pages,
            asked: strings(state.get("asked"))?,
            found,
        })
    }

    /// Takes in the answers to the requests asked last round. A query the
    /// store refused is returned as a gate error; a missing answer or one of
    /// the wrong shape is a malformed host input.
    pub fn absorb(
        &mut self,
        batch: &Batch,
        plan: &[Query],
        results: &Value,
    ) -> Result<Vec<String>, String> {
        let results = results
            .as_object()
            .ok_or("data_results must be an object keyed by request id")?;
        let mut failures = Vec::new();
        for id in std::mem::take(&mut self.asked) {
            let (index, offset) = parse_request_id(&id)
                .filter(|(index, _)| *index < plan.len())
                .ok_or_else(|| format!("state names request {id:?}, which this batch never asks"))?;
            let lookup = plan[index].lookup;
            let answer = results
                .get(&id)
                .ok_or_else(|| format!("the host returned no answer for data request {id}"))?;
            if let Some(error) = answer.get("error") {
                failures.push(format!(
                    "ERROR: store query on {} failed: {}",
                    lookup.collection(),
                    error.as_str().unwrap_or("unknown error")
                ));
                continue;
            }
            let rows = answer
                .get("result")
                .and_then(|r| r.get("rows"))
                .and_then(Value::as_array)
                .ok_or_else(|| format!("data request {id} answered without result.rows"))?;
            if lookup == Lookup::Profile {
                self.found.profile = rows.first().cloned();
                continue;
            }
            if matches!(lookup, Lookup::Source | Lookup::SourceText) {
                for row in rows {
                    self.found.absorb_source(batch, row);
                }
                if rows.len() >= CHUNK {
                    self.pages.push((index, offset + CHUNK as u32));
                }
                continue;
            }
            for row in rows {
                let Some(value) = row.get(lookup.field()).and_then(Value::as_str) else {
                    continue;
                };
                if let (Lookup::Claim, Some(kind)) =
                    (lookup, row.get("kind").and_then(Value::as_str))
                {
                    self.found
                        .claim_kinds
                        .insert(value.to_owned(), kind.to_owned());
                }
                if let Some(set) = self.found.set(lookup) {
                    set.insert(value.to_owned());
                }
            }
            if rows.len() >= CHUNK {
                self.pages.push((index, offset + CHUNK as u32));
            }
        }
        Ok(failures)
    }

    /// The next round's requests, or `None` when everything is answered.
    pub fn next_requests(&mut self, graph_id: &str, plan: &[Query]) -> Option<Vec<Value>> {
        let mut asks: Vec<(usize, u32)> = Vec::new();
        while asks.len() < PER_ROUND && !self.pages.is_empty() {
            asks.push(self.pages.remove(0));
        }
        while asks.len() < PER_ROUND && self.cursor < plan.len() {
            asks.push((self.cursor, 0));
            self.cursor += 1;
        }
        if asks.is_empty() {
            return None;
        }
        self.asked = asks.iter().map(|(i, o)| request_id(*i, *o)).collect();
        Some(
            asks.iter()
                .map(|(index, offset)| {
                    let query = &plan[*index];
                    let mut body = json!({
                        "collection": query.lookup.collection(),
                        "filter": query.lookup.filter(graph_id, &query.ids),
                        "limit": CHUNK,
                        "fields": query.lookup.fields(),
                    });
                    if query.lookup == Lookup::Profile {
                        body["order"] = json!({"version": "DESC"});
                        body["limit"] = json!(1);
                    }
                    if query.lookup == Lookup::SourceText {
                        body["limit"] = json!(TEXT_CHUNK);
                    }
                    if *offset > 0 {
                        body["offset"] = json!(offset);
                    }
                    json!({"id": request_id(*index, *offset), "query": body})
                })
                .collect(),
        )
    }
}
