# claim_graph pack

Append-only claim graphs kept as DefraDB documents, admitted only through a mechanical gate. A `Claim` is one
sourced statement with a verbatim quote from the narrative file or fetched source it came from; an `Edge` relates two claims; an
`Entity` is a thing claims link to; a `Perspective` is an issuer (a person, a position, a critic). One node may
hold several graphs, each named by a `graph_id` and each with its own vocabulary, the graph's profile
(`ClaimGraph`). Writers never create graph rows directly: they create a `ClaimBatch` holding JSONL text, and
the `claim_gate` plugin checks the whole batch against the graph's profile (the checks of a research graph's
`validate.sh` and `quotes.sh`, plus the dispute rules) and, in one transaction, writes a `GateReceipt` and the
admitted rows, or only a failing receipt. A quote is checked against the narrative file it names or, for a claim made from a fetched web page, against that page's stored `Source` text. `claim_sources` fetches a URL into a `Source` document. `claim_views` derives a graph's effective status, open disputes and
per-perspective reputation as Markdown. `claim_export` writes a graph back as the same JSONL files. Three agents work on the graphs. The **Claim graph researcher** (`claim-researcher`) takes a `ResearchRequest` naming a topic, starts the graph if it does not exist, fetches sources, files claims whose quotes come verbatim from them, fixes refused batches, and closes the request with a `ResearchReceipt`. The **Claim graph reviewer** (`claim-reviewer`) then checks a sample of those claims against their quotes and files reviews and challenges. The **Claim graph expert** (`claim-sme`) answers questions from a graph with a claim id on every statement, files the questions it cannot answer, and raises challenges, reviews and replies when asked.

```text
ClaimBatch (created) --claim-batch--> claim-gate (claim_gate plugin)
                                         |-- receipt      --> GateReceipt (always one)
                                         |-- claims       --> Claim       (on pass)
                                         |-- edges        --> Edge        (on pass)
                                         |-- entities     --> Entity      (on pass)
                                         |-- perspectives --> Perspective (on pass)
                                         `-- profile      --> ClaimGraph  (on pass, when the batch carries one)

ViewRequest (created) --view-request--> claim-views (claim_views plugin)
                                          `-- receipt --> ViewReceipt (always one)

ExportRequest (created) --export-request--> claim-export (claim_export plugin)
                                              `-- receipt --> ExportReceipt (always one)

SourceRequest (created) --source-request--> claim-sources (claim_sources plugin)
                                              `-- source --> Source (always one; status ok or failed)

ResearchRequest (created) --research-request--> claim-research-task (claim-researcher)
                                                  writes ClaimBatch, SourceRequest, then one ResearchReceipt
ResearchReceipt (created) --research-receipt--> claim-review-task (claim-reviewer)
                                                  writes ClaimBatch (reviews, challenges), ViewRequest
```

## Installation

```sh
gents pack install ./packs/gents/claim_graph --home <home> --grant-authority \
  --inference-slot sme=<profile> --inference-slot researcher=<profile> --inference-slot reviewer=<profile>
gents pack install gents/claim_graph --home <home> --grant-authority ...   # once published
```

`--grant-authority` allows `claim_sources` its outbound HTTP (below); without the grant every `Source` comes
back `failed` with the reason, and the rest of the pack works unchanged.

Building from a source checkout compiles the four plugins to `wasm32-wasip1` (`gents pack build`), so it
needs a Rust toolchain with `rustup target add wasm32-wasip1`; a pack fetched pre-built skips the build.

## Bindings and prerequisites

- Three inference slots: `sme` (the `claim-sme` agent), `researcher` (`claim-researcher`; a model that follows
  JSON tool schemas reliably, since every write is JSONL text inside a tool argument) and `reviewer`
  (`claim-reviewer`). The gate, the fetcher, the views and the export need no model.
- Outbound HTTPS for `claim_sources` (Authority, below). The research flow needs no folder: agent batches are
  quote-checked against stored sources, and the tool ceiling can stay `readonly`.
- An allowed folder for the gate's file and quote checks: `source_root` must be an absolute path inside a
  folder added with `gents plugin dirs add <folder> --home <home>`. A callback has nobody to ask, so a
  `source_root` outside the allowed folders fails the call. For a research graph that lives at
  `<root>/graph/`, the source root is `<root>`.
- A writable allowed folder for export and for view files: `gents plugin dirs add <out> --home <home> --access
  read_write`.
- A graph exists once a batch carrying its profile passed the gate. The SME's question recipe needs the
  profile's `question_entity` and its `default_asker` perspective to exist in the graph; importing a research
  graph with `profiles/research.json` brings both.

## Authority

| Who | May | Why |
| --- | --- | --- |
| `claim_gate` (callback) | query `Claim`, `Edge`, `Entity`, `Perspective`, `ClaimGraph`, `Source`; read `source_root` | profile lookup, existence and uniqueness checks against the store; file and verbatim-quote checks against files and stored sources. Its rows reach the store only as the callback's declared outputs, committed with the receipt |
| `claim_sources` (callback) | outbound HTTP (`OutboundHttp`, unscoped); query `Source` | fetches one URL per `SourceRequest` after checking whether the graph already holds it; its only database write is the `Source` row. No folder, no environment |
| `claim_views` (callback) | query `Claim`, `Edge`, `Entity`, `Perspective`, `ClaimGraph`; read and write `out_dir` | derives a view; its only database write is the `ViewReceipt` |
| `claim_export` (tool and callback) | query the gate's six collections; read and write `out_dir` | pages one graph and writes the JSONL tree; as the `claim-export` callback its only database write is the `ExportReceipt` |
| `claim-researcher` (agent) | `query` over `ClaimGraph`, the four graph collections, `GateReceipt`, `Source`, `ResearchRequest`; `start_graph`, `file_batch`, `fetch_source`, `finish_research` | writes only `ClaimBatch` (through the gate), `SourceRequest` (through the fetcher) and its `ResearchReceipt`; `graph_id`, and the receipt's `request_id`, are filled from the request it runs for, so it cannot write into another graph |
| `claim-reviewer` (agent) | `query` over `ClaimGraph`, the four graph collections, `GateReceipt`, `Source`, `ResearchReceipt`, `ViewReceipt`; `file_batch`, `request_view` | writes only a `ClaimBatch` and a `ViewRequest`, with `graph_id` filled from the receipt it reviews |
| `claim-sme` (agent) | `query` over the four graph collections, `ClaimGraph`, `Source`, `GateReceipt`, `ViewReceipt`, `ExportReceipt` and `ResearchReceipt`; `file_claims`; `request_view`; `start_research`; `claim_export` | retrieval and status derivation; it writes only a `ClaimBatch`, which still goes through the gate, a `ViewRequest`, and a `ResearchRequest`, which hands the topic to the researcher |

Only `claim_sources` needs the network, and nothing in the research flow needs a folder; no agent surface
exposes `source_root`, `out_dir` or a path. The host's HTTP client follows no redirects (the plugin follows up
to five itself, 8 s each), and an unscoped `OutboundHttp` grant reaches only `https://` on port 443: `http://`
URLs and other ports are refused unless the operator installs with an explicit allow-list.

Nothing in the pack updates or deletes a graph row: every scalar field of every collection is `@immutable`
(DefraDB refuses the directive on list fields: `Claim.entities`, `Entity.aliases` and the vocabulary lists of
`ClaimGraph`, which are never updated by convention), and a wrong claim is answered with a new claim and a
`retracts` or `supersedes` edge. A profile changes by admitting a new version.

## Graphs and profiles

Every row carries `graph_id`. Graph rows are unique per graph, not per node: `Claim` on `(graph_id, claim_id)`,
`Edge` on `(graph_id, edge_key)`, `Entity` on `(graph_id, entity_id)`, `Perspective` on `(graph_id,
perspective_id)`, so two graphs may use the same ids. `ClaimBatch.batch_id`, the receipts' and requests' ids
stay unique across the node, because they are correlation keys.

A `ClaimGraph` row is one version of a graph's vocabulary, unique on `(graph_id, version)`:

| Field | Meaning |
| --- | --- |
| `kinds`, `confidence_classes`, `edge_types` | the allowed claim kinds, confidence classes and edge types |
| `mapping_targets`, `mapping_fits` | the allowed `against` and `fit` of a `mapping` claim |
| `review_verdicts` | the allowed `verdict` of a `review` claim |
| `resolver_perspectives` | the perspectives whose `resolves` edges close challenges and adjudicate reviews |
| `question_entity`, `default_asker`, `questions_from_prefix` | what filed questions link to, who asks by default, and the `from` prefix that exempts a question from the quote check |
| `claim_id_pattern`, `text_max_words` | the claim id regex and the word limit of `text` (1 to 400) |
| `display_name`, `note`, `created_at` | description |
| `version`, `batch_id` | assigned by the gate: one more than the graph's highest version, and the batch that carried it |

A batch's optional `profile` field holds a profile as JSON text (every field above except `graph_id`,
`version`, `batch_id`, `created_at`). The gate validates it (non-empty lists of distinct strings, a compilable
`claim_id_pattern`, `text_max_words` in range, every resolver a perspective in the batch or the store), checks
the batch against it, and on pass admits it as the next version. A batch without a profile is checked against
the latest version; the first batch of a graph must carry one, else it fails with `ERROR: unknown graph
<graph_id>: the first batch must carry a profile`.

`profiles/research.json` is the research graph's vocabulary with the dispute kinds and edges added:

```json
{"display_name": "Government attestation research", "kinds": ["fact", "number", "date", "rule", "mapping", "interpretation", "position", "quote", "question", "challenge", "review"], "confidence_classes": ["sourced", "memory", "inferred"], "edge_types": ["supports", "contradicts", "supersedes", "retracts", "answers", "maps-to", "said-first", "depends-on", "challenges", "reviews", "resolves"], "mapping_targets": ["defradb", "fefra"], "mapping_fits": ["native", "partial", "gap", "convention"], "review_verdicts": ["sustains", "weakens", "unclear"], "resolver_perspectives": ["p:mike"], "question_entity": "e:open-question", "default_asker": "p:mike", "questions_from_prefix": "graph/claims/questions.jsonl", "claim_id_pattern": "^c:[a-z0-9-]+:[0-9]{3}$", "text_max_words": 40, "note": "..."}
```

`profiles/general.json` is the vocabulary the researcher starts a new graph with: the kinds `fact number date
rule interpretation position quote question challenge review`, all eleven edge types, no mapping targets or
fits, the verdicts `sustains weakens unclear`, `p:owner` as resolver and default asker, `e:open-question` as
question entity, and `memory:questions` as the questions prefix, so a question filed from memory carries
`confidence: memory` (the research profile's file-based questions keep `inferred`).

## Effective status

Status is never stored authoritatively. A claim's `status` field is its writer's advice; readers (the views
plugin, the SME) derive the effective status of a claim X from the edges, the first that holds:

1. `retracted`: a `retracts` edge points at X.
2. `superseded`: a `supersedes` edge points at X.
3. `contested`: a `contradicts` edge touches X (either end) and no `supersedes` edge joins the same two claims.
4. `challenged`: a claim C of kind `challenge` has a `challenges` edge to X, and no `answers` or `resolves`
   edge points at C.
5. `live`.

A `resolves` edge pointing at a challenge closes it; one pointing at a review marks the review adjudicated and
has no effect on the review's target. Reviews never change status.

## Disputes and reputation

A challenge is a claim of kind `challenge` stating an objection, filed with exactly one `challenges` edge from
it to its target. A review is a claim of kind `review` with a `verdict` from the profile's `review_verdicts`,
filed with exactly one `reviews` edge to its target. Anyone replies to a challenge with a claim and an
`answers` edge to the challenge, which closes it; a perspective in `resolver_perspectives` closes a challenge or
adjudicates a review with a `resolves` edge. A dispute claim whose `quote` equals its `text` is exempt from the
quote check. These rules apply only when the profile lists the kinds and edge types; a profile may leave
disputes out.

Reputation is derived, never stored: the `reputation` view counts, per perspective, the claims asserted (by
confidence), those later retracted or superseded, the challenges raised and how many were closed, the
challenges received and the reviews given by verdict. It computes no score; a weighting belongs to whoever
reads the table.

## Inputs and outputs

### ClaimBatch

| Field | Meaning |
| --- | --- |
| `batch_id` | unique correlation key, by convention `<graph_id>-<section>-<yyyymmdd>-<n>`; every admitted row and the receipt carry it |
| `graph_id` | the graph every row of the batch joins |
| `section` | the claims file stem the claims belong to (`datapack-flipper`); stored on each `Claim` |
| `profile` | optional JSON text of a profile to admit as the graph's next version (above) |
| `claims`, `edges`, `entities`, `perspectives` | JSONL text, one JSON object per line, any subset |
| `source_root` | optional; when present, `from` files, cited `sources[].doc` files and verbatim quotes are checked under it; when absent they are skipped and the receipt counts them in `quotes_skipped` |
| `extractor`, `sample_size`, `failure_count`, `note`, `submitted_at` | provenance of the batch; not checked |

Line shapes (the research JSONL shapes, keys in this order):

| Line | Keys |
| --- | --- |
| claim | `id, text, quote, from, sources, entities, kind, by, asserted, status, confidence, note`, a review also `verdict`, mapping claims also `fit, against, ref`, optional `valid_from, valid_to` |
| edge | `from, to, type, by, date, note` |
| entity | `id, type, name, aliases, note` |
| perspective | `id, name, kind, note`, optional `did` |

Stored rows rename the GraphQL-sensitive keys: a claim's `id` is `claim_id`, `from` is `from_ref`, `sources` is
the JSON text of the array; an edge is `from_claim`, `edge_type`, `to_claim` with
`edge_key = "<from_claim>|<edge_type>|<to_claim>"`; an entity is `entity_id`, `entity_type`; a perspective is
`perspective_id`. `Claim.entity_text` is the entity ids space-joined and `Entity.alias_text` the lowercased name
and aliases, so a `_like` filter finds them. `Claim.text` and `Entity.name` carry BM25 full-text indexes.

### Gate rules

Any error fails the whole batch. Every existence check is within the batch's graph.

1. Every line is a JSON object. Claim ids match the profile's `claim_id_pattern`, are unique in the batch and
   absent from the graph.
2. A claim has `text, quote, from, sources, entities, kind, by, asserted, status, confidence`; `text` has at most
   `text_max_words` words; `kind` is in `kinds`; `status` one of `live superseded contested retracted`;
   `confidence` in `confidence_classes`. A mapping claim has `fit` in `mapping_fits` and `against` in
   `mapping_targets`. At least one entity. A `sourced` claim cites a source with a `doc` or `code`; a `code`
   reference is `path[:line[-line]]`.
3. Every entity a claim names, and every `by`, exists in the batch or the graph; entity and perspective ids in
   the batch are unique and new.
4. An edge has `from, to, type, by, date`; `type` is in `edge_types`; both ends exist in the batch or the graph;
   its `edge_key` is new.
5. Disputes: a `challenge` or `review` claim comes with exactly one `challenges` or `reviews` edge from it in
   the same batch; such an edge starts at a claim of that kind; a review's `verdict` is in `review_verdicts`; a
   `resolves` edge's `by` is in `resolver_perspectives`.
6. With `source_root`: `from` (before `#`) and every cited `doc` exist under it, and every `quote` occurs
   verbatim in its `from` file, except claims filed under `questions_from_prefix`, `question`, `challenge` and
   `review` claims whose `quote` equals their `text`, and claims a `supersedes` or `retracts` edge (in the
   batch or the store) points at.
7. Stored sources: a `from` of `source:<source_id>#<anchor>` names a `Source` of the graph with `status` `ok`,
   and the `quote` occurs verbatim in its `text` (the same matching as files; counted in `quotes_checked`); each
   `sources[].source` names such a `Source`, and one satisfies `confidence: sourced` as a `doc` or `code` does.
   A `from` of `memory:<anything>` needs `quote` equal to `text` and `confidence: memory`. The errors read
   `ERROR: <id> cites source <source_id> which is missing or failed`, `ERROR: <id> quote not found verbatim in
   source <source_id>`, `ERROR: <id> from names no source id (write source:<source_id>#<anchor>)`, `ERROR: <id>
   is from memory but claims confidence <c>`, `ERROR: <id> is from memory but its quote is not its text` and
   `ERROR: <id> is sourced but cites no fetched doc, code ref or source`.

### GateReceipt

| Field | Meaning |
| --- | --- |
| `receipt_id` | `receipt-<batch_id>` |
| `graph_id` | the batch's graph |
| `verdict` | `pass` or `fail` |
| `errors` | JSON text of an array of `ERROR: ...` strings, at most 200 |
| `claims_admitted`, `edges_admitted`, `entities_admitted`, `perspectives_admitted` | rows written |
| `quotes_checked`, `quotes_skipped` | quote checks run and skipped |
| `profile_version` | the profile version the batch was checked against |
| `gate_version`, `checked_at` | the plugin version and an ISO-8601 UTC time |

### Filing claims

The SME's `file_claims` tool creates a `ClaimBatch` with `batch_id`, `graph_id`, `section`, `claims` and
optionally `edges`; it cannot set `source_root` or `profile`, so the gate skips the file checks (stored-source
quotes are still checked). A
question is one claim line, such as:

```json
{"id": "c:q:014", "text": "Which body issues Estonian e-seal certificates?", "quote": "Which body issues Estonian e-seal certificates?", "from": "graph/claims/questions.jsonl#questions", "sources": [], "entities": ["e:open-question"], "kind": "question", "by": "p:mike", "asserted": "2026-10-29", "status": "live", "confidence": "inferred", "note": "Searched e-seal and trust service; no issuer claim"}
```

A challenge is a `challenge` claim plus `edges` holding its `challenges` edge, for example
`{"from": "c:ch:001", "to": "c:eu:010", "type": "challenges", "by": "p:mike", "date": "2026-10-29", "note": ""}`;
a review is a `review` claim with `"verdict"` after `note` plus its `reviews` edge; a reply is a claim plus an
`answers` edge to the challenge. The prompt (`agent_behaviors/claim_sme/system_prompt.md`) holds the id and
batch conventions. In a graph started with `profiles/general.json` the question's `from` is
`memory:questions#questions` and its `confidence` is `memory`.

### Research

A `ResearchRequest` starts the research flow, made in one of two ways:

- From a chat: ask the expert for research on a topic. Its `start_research` tool creates the request with
  `request_id`, `graph_id` (the graph in use, or a new slug for a topic no graph covers), `topic` and an
  optional `brief`, and the expert replies with the `request_id`; ask it later how the research went and it
  reads the `ResearchReceipt`.
- By document: `gents document create ResearchRequest --home <home> --graphql <url> --json '{"request_id":
  "flipper-fcc-1", "graph_id": "flipper-fcc", "topic": "..."}'`, or a seed as in the live scenario.

The `research-request` trigger hands it to `claim-researcher`, serially, one request
at a time:

| `ResearchRequest` field | Meaning |
| --- | --- |
| `request_id` | unique correlation key; every batch, source request and the receipt the researcher writes start with it |
| `graph_id` | required: the graph to research into; every researcher write is filled with it |
| `topic`, `brief` | what to research, and what the answer should cover |
| `max_sources` | the most URLs to fetch for the request; unset means 4 |
| `create_graph` | advice only: the researcher starts the graph exactly when no `ClaimGraph` exists for `graph_id` |
| `note`, `submitted_at` | provenance |

The researcher (`agent_behaviors/claim_researcher/system_prompt.md`) starts a missing graph with
`profiles/general.json`, the perspectives `p:owner` (person), `p:researcher` (position) and `p:reviewer` (critic)
and the entity `e:open-question`; plans 3 to 6 sections; fetches sources (`fetch_source` creates a
`SourceRequest`, the `claim-sources` callback answers with a `Source`); files one batch per section with claims
whose `from` is `source:<source_id>#p<page>` and whose quote is copied from that page of `Source.text`; reads
each `GateReceipt` and refiles a failed batch as `<batch_id>-r2`, `-r3`; files its open questions; and writes
one `ResearchReceipt`:

| `ResearchReceipt` field | Meaning |
| --- | --- |
| `receipt_id`, `request_id`, `graph_id` | the receipt, its request (filled from the trigger) and its graph |
| `summary` | the sections with their claim counts and the open questions |
| `sources_fetched`, `sources_failed` | `Source` rows ok and failed |
| `batches_filed`, `claims_admitted`, `claims_memory` | batches filed (refiles included), claims admitted, of which from memory |
| `open_questions`, `finished_at` | the question claim ids, space-separated, and an ISO-8601 UTC time |

The receipt runs `claim-reviewer` (`research-receipt` trigger, serial), which reads up to five of the request's
sourced claims, judges whether each quote supports its text, and files one batch of `review` claims (verdict
`sustains`, `weakens` or `unclear`) and `challenge` claims, each with its edge, `from` `review:<target id>`,
`confidence: inferred`.

No-web mode: when no source can be fetched (no network grant, every URL refused), the researcher files what it
knows as memory claims (`from` `memory:<section>`, `quote` equal to `text`, `confidence: memory`) and says so in
the summary; the gate refuses a `memory:` claim marked `sourced`, so such a graph never claims a checked quote.

### Sources

A `SourceRequest` (`request_id`, `graph_id`, `url`, `confidential`, `note`, `submitted_at`) runs the
`claim-sources` callback, which writes exactly one `Source` with the same `request_id`. A fetch that fails is a
`Source` with `status: failed` and the reason in `error`, never a missing row.

| `Source` field | Meaning |
| --- | --- |
| `source_id` | `s:` and 16 hex digits of the URL's hash for the first successful fetch of a URL; a refetch or a failure gets a longer id (`s:<16 hex>-<8 hex>`), so readers copy it whole and assume no shape |
| `graph_id`, `request_id` | its graph and the request that fetched it |
| `url`, `final_url`, `title`, `fetched_at` | what was asked, where redirects led, the page title, an ISO-8601 UTC time |
| `status`, `error`, `http_status` | `ok` or `failed`, the reason, the HTTP status |
| `content_type`, `content_hash` | the response type and `sha256:<hex>` of the raw body |
| `text`, `text_chars`, `truncated` | the plain text (HTML stripped, plain text and JSON verbatim), at most 200 000 characters |
| `confidential`, `note` | copied from the request |

A URL the graph already holds comes back `failed` with an `error` naming the stored `source_id` to cite. See
`plugins/claim_sources/TOOL.md`.

### Views

A `ViewRequest` runs the `claim-views` callback, which writes one `ViewReceipt`. The SME creates one with
`request_view`; an operator with `gents document create ViewRequest`.

| `ViewRequest` field | Meaning |
| --- | --- |
| `request_id` | unique correlation key; the receipt carries it |
| `graph_id` | the graph to read |
| `view` | `status` (counts per effective status, then every non-live claim with its deciding edge), `disputes` (open challenges with target, challenger, days open and the target's reviews) or `reputation` (the per-perspective counts above) |
| `out_dir` | optional absolute path inside a folder allowed `read_write`; the Markdown is also written to `<out_dir>/<graph_id>/<view>.md` |
| `dry_run` | write no file |
| `note`, `submitted_at` | provenance; not passed to the plugin |

| `ViewReceipt` field | Meaning |
| --- | --- |
| `receipt_id`, `request_id`, `graph_id`, `view` | the receipt and its request |
| `ok`, `error` | whether the view was derived, and the sentence it failed with |
| `rows` | the rows the view lists |
| `markdown` | the view, inline, up to 60 000 characters (truncated with a final line saying so) |
| `file`, `generated_at` | the file written, if any, and an ISO-8601 UTC time |

See `plugins/claim_views/TOOL.md`.

### Export

`claim_export` with `{"graph_id": "<graph>", "out_dir": "<allowed folder>"}` writes `claims/<section>.jsonl`,
`edges/edges.jsonl`, `entities/entities.jsonl`, `perspectives/perspectives.jsonl`, each sorted by id, and the
graph's profile versions as `claim_graph.json` (a JSON array), and returns the counts; `{"graph_id": "<graph>",
"dry_run": true}` returns the counts only. It also writes the graph's sources as `sources/sources.jsonl`, metadata only
unless `include_source_text` is true. See `plugins/claim_export/TOOL.md`.

An operator exports without a model by creating an `ExportRequest`; the `claim-export` callback runs the plugin
and writes one `ExportReceipt`. `gents plugin run --bind-dir` binds its folder read-only, so it cannot write the
export; the callback is bound read-write through the allowed folder instead:

```sh
gents plugin dirs add /path/to/out --home <home> --access read_write
gents document create ExportRequest --home <home> --graphql http://127.0.0.1:8080/api/v0/graphql \
  --json '{"request_id": "export-20261029-1", "graph_id": "government-attestation", "out_dir": "/path/to/out"}'
gents query find --home <home> --graphql http://127.0.0.1:8080/api/v0/graphql --collection ExportReceipt \
  --filter '{"request_id": {"_eq": "export-20261029-1"}}' --field ok --field error --field files_json
```

| `ExportRequest` field | Meaning |
| --- | --- |
| `request_id` | unique correlation key; the receipt carries it |
| `graph_id` | the graph to export |
| `out_dir` | absolute path inside a folder allowed `read_write`; may be left out with `dry_run` |
| `sections` | JSON text of an array of claim section names; absent means every section |
| `dry_run` | count the rows and write nothing |
| `include_source_text` | also write each `Source`'s `text` into `sources/sources.jsonl` |
| `note`, `submitted_at` | provenance; not passed to the plugin |

| `ExportReceipt` field | Meaning |
| --- | --- |
| `receipt_id`, `request_id`, `graph_id` | the receipt, its request and its graph |
| `ok`, `error` | whether the export finished, and the sentence it failed with |
| `claims_written`, `edges_written`, `entities_written`, `perspectives_written` | rows written (counted, with `dry_run`) |
| `sections_json`, `files_json` | JSON text of the sections exported and the files written |
| `dry_run`, `exported_at` | the mode and an ISO-8601 UTC time |

## Importing a research graph

`tools/import_jsonl.sh` is operator tooling, not a plugin. Against a running server:

```sh
gents plugin dirs add /path/to/research --home <home>
gents server --home <home> --http-port 8080 --p2p-transport none &
packs/gents/claim_graph/tools/import_jsonl.sh --graph-id government-attestation \
  --profile packs/gents/claim_graph/profiles/research.json --source-root \
  /path/to/research/graph <home> http://127.0.0.1:8080/api/v0/graphql
```

It needs `jq` and `gents` (`GENTS` overrides the binary) and creates the batches with
`gents document create ClaimBatch`, waiting for each `GateReceipt` (`gents query find`) before the next:
perspectives and every `entities/*.jsonl` first, then each `claims/<section>.jsonl`, then the edges. Every
batch carries `--graph-id` (required) and its `batch_id` starts with it, so two graphs import side by side.
`--profile FILE` attaches the profile to the first batch, which carries every perspective, so the profile's
resolvers can be checked; a graph's first import needs it, and a later import with it admits a new version.
A section larger than `--chunk-bytes` (default 100000, under Linux's 128 KiB limit for one argument) is split
into several batches. A `supersedes` or `retracts` edge travels in the batch of the claim it points at, and the
sections are ordered so the superseding claim is admitted first, because the gate skips the quote check of a
superseded claim only when such an edge exists.

Running it again resumes. Each planned batch is a family of attempts: `<batch_id>`, then `<batch_id>-r2`,
`<batch_id>-r3` and so on. A family with a passing receipt is skipped. An attempt that has no receipt yet is
waited for, not resubmitted. A family whose attempts all failed in an earlier run is submitted again as
`<batch_id>-rN`, N being one more than its attempts so far (`retrying <batch_id> as <batch_id>-rN`). A failure
in the current run stops the import with the gate's errors; fix the input and run it again. `--suffix` starts
new families under new batch ids. `--dry-run` prints the plan, one line per batch with its `graph_id` and
whether it carries the profile. `--source-root=<dir>` names another source root; without the flag the file and
quote checks are skipped. An edge that repeats an earlier `(from, type, to)` is dropped with a warning naming
both lines, since the gate refuses an edge key twice; `--strict` stops instead.

## Completion and failure

A batch is complete when its `GateReceipt` exists. On `pass` its rows, and its profile version if it carried
one, were committed in the same transaction as the receipt; on `fail` nothing else was written and `errors`
says why. A view or an export is complete when its receipt exists; `ok: false` carries the reason. A malformed
host input (not a validation failure) fails the callback call itself, and no receipt appears; the server log
names the reason. A `batch_id` is used once: correct a failed batch and submit it under a new id (the import
script does this itself, as `<batch_id>-rN`).

## Migration

Rows written by the first version of this pack carry no `graph_id` and no profile, so the gate, the views, the
export and the SME (which all scope by `graph_id`) do not read them. There are none in the wild: the first
version was never imported outside test homes. Import the research graph again with `--graph-id`.

0.3.0 adds collections (`Source`, `SourceRequest`, `ResearchRequest`, `ResearchReceipt`) and the optional
`ExportRequest.include_source_text`; it changes no existing field, so a 0.2.0 home upgrades by reinstalling the
pack, with no re-init and no re-import.

## Validation

```sh
gents pack check packs/gents/claim_graph
scripts/test-pack.sh packs/gents/claim_graph
```

The suite builds the pack, runs every plugin case (`plugins/*/tests/*.json`) and each plugin's `cargo test`,
the install case (`tests/install.json`: thirty documents, four plugins, the `sme`, `researcher` and `reviewer`
slots), the configuration checks in `tests/content.json` (among them: each prompt's schema card names exactly the
SDL fields of the collections it describes, and the worked batches parse with the general profile verbatim),
the scenario seam checks in `tests/scenario.json`, and seven runtime cases served from a throwaway repository:

- `tests/gate_runtime.json` seeds a valid batch with an inline profile whose quote and cited document are in the
  repository, and expects a passing receipt at profile version 1, one `Claim`, `Entity` and `Perspective`, and
  the `ClaimGraph` row.
- `tests/gate_runtime_fail.json` seeds a batch with an unknown kind, unknown entity and perspective, a missing
  quote and a dangling edge, and expects a failing receipt with nothing admitted.
- `tests/dispute_runtime.json` seeds a profile, two perspectives, an entity, a claim and a challenge with its
  `challenges` edge, and expects a passing receipt with two claims and one edge, and the challenge row.
- `tests/view_runtime.json` seeds a dry-run `status` `ViewRequest` for a graph with no rows and expects an `ok`
  `ViewReceipt` with no rows.
- `tests/export_runtime.json` seeds a dry-run `ExportRequest` and expects an `ExportReceipt` with `ok` and
  `dry_run`.
- `tests/source_runtime.json` seeds a `SourceRequest` for `http://127.0.0.1:9/` and expects a `Source` with
  `status: failed`.
- `tests/memory_claim_runtime.json` seeds a batch whose `memory:` claim is marked `sourced` and expects a failing
  receipt with nothing admitted.

The live scenario `scenarios/claim_graph_live` (repository root) runs the research flow with a real model:

```sh
make live LIVE_PACK=claim_graph LIVE_ENDPOINT=http://127.0.0.1:11434/v1 LIVE_MODEL=qwen3.8:latest
```

It seeds one `ResearchRequest` (topic: the Flipper Zero's FCC certification) and passes when the expert's answer
from the new graph completes and the home holds at least one `Source`, five `Claim`s, one `GateReceipt` and one
`ResearchReceipt`.

Research data the import tolerates: the research `edges/edges.jsonl` gives `c:exchange:084|supports|c:pitch:164`
twice (lines 67 and 181, with different `date` and `note`); the import keeps line 67 and warns about line 181.
Fix the source file to import it with `--strict`.

## Operational history

29 Oct 2026: pack created. 29 Oct 2026: profiles, several graphs per node, disputes and views added. 30 Oct
2026: 0.3.0, sources as documents, the researcher and reviewer agents and the live research scenario added. No
run recorded yet.
