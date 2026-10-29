# claim_graph pack

Append-only claim graphs kept as DefraDB documents, admitted only through a mechanical gate. A `Claim` is one
sourced statement with a verbatim quote from the narrative file it came from; an `Edge` relates two claims; an
`Entity` is a thing claims link to; a `Perspective` is an issuer (a person, a position, a critic). One node may
hold several graphs, each named by a `graph_id` and each with its own vocabulary, the graph's profile
(`ClaimGraph`). Writers never create graph rows directly: they create a `ClaimBatch` holding JSONL text, and
the `claim_gate` plugin checks the whole batch against the graph's profile (the checks of a research graph's
`validate.sh` and `quotes.sh`, plus the dispute rules) and, in one transaction, writes a `GateReceipt` and the
admitted rows, or only a failing receipt. `claim_views` derives a graph's effective status, open disputes and
per-perspective reputation as Markdown. `claim_export` writes a graph back as the same JSONL files. A **Claim
graph expert** agent (`claim-sme`) answers questions from a graph with a claim id on every statement, files
the questions it cannot answer, and raises challenges, reviews and replies when asked.

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
```

## Installation

```sh
gents pack install ./packs/gents/claim_graph --home <home> --inference-slot sme=<profile>
gents pack install gents/claim_graph --home <home> --inference-slot sme=<profile>   # once published
```

Building from a source checkout compiles the three plugins to `wasm32-wasip1` (`gents pack build`), so it
needs a Rust toolchain with `rustup target add wasm32-wasip1`; a pack fetched pre-built skips the build.

## Bindings and prerequisites

- One inference slot, `sme`, bound to the `claim-sme` agent. Any capable profile. The gate and the views need
  no model.
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
| `claim_gate` (callback) | query `Claim`, `Edge`, `Entity`, `Perspective`, `ClaimGraph`; read `source_root` | profile lookup, existence and uniqueness checks against the store; file and verbatim-quote checks. Its rows reach the store only as the callback's declared outputs, committed with the receipt |
| `claim_views` (callback) | query the same five collections; read and write `out_dir` | derives a view; its only database write is the `ViewReceipt` |
| `claim_export` (tool and callback) | query the same five collections; read and write `out_dir` | pages one graph and writes the JSONL tree; as the `claim-export` callback its only database write is the `ExportReceipt` |
| `claim-sme` (agent) | `query` over the four graph collections, `ClaimGraph`, `GateReceipt`, `ViewReceipt` and `ExportReceipt`; `file_claims`; `request_view`; `claim_export` | retrieval and status derivation; it writes only a `ClaimBatch`, which still goes through the gate, and a `ViewRequest` |

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
optionally `edges`; it cannot set `source_root` or `profile`, so the gate skips the file and quote checks. A
question is one claim line, such as:

```json
{"id": "c:q:014", "text": "Which body issues Estonian e-seal certificates?", "quote": "Which body issues Estonian e-seal certificates?", "from": "graph/claims/questions.jsonl#questions", "sources": [], "entities": ["e:open-question"], "kind": "question", "by": "p:mike", "asserted": "2026-10-29", "status": "live", "confidence": "inferred", "note": "Searched e-seal and trust service; no issuer claim"}
```

A challenge is a `challenge` claim plus `edges` holding its `challenges` edge, for example
`{"from": "c:ch:001", "to": "c:eu:010", "type": "challenges", "by": "p:mike", "date": "2026-10-29", "note": ""}`;
a review is a `review` claim with `"verdict"` after `note` plus its `reviews` edge; a reply is a claim plus an
`answers` edge to the challenge. The prompt (`agent_behaviors/claim_sme/system_prompt.md`) holds the id and
batch conventions.

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
"dry_run": true}` returns the counts only. See `plugins/claim_export/TOOL.md`.

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

## Validation

```sh
gents pack check packs/gents/claim_graph
scripts/test-pack.sh packs/gents/claim_graph
```

The suite builds the pack, runs every plugin case (`plugins/*/tests/*.json`) and each plugin's `cargo test`,
the install case (`tests/install.json`: thirteen documents, three plugins, the `sme` slot), the configuration
checks in `tests/content.json`, and five runtime cases served from a throwaway repository:

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

Research data the import tolerates: the research `edges/edges.jsonl` gives `c:exchange:084|supports|c:pitch:164`
twice (lines 67 and 181, with different `date` and `note`); the import keeps line 67 and warns about line 181.
Fix the source file to import it with `--strict`.

## Operational history

29 Oct 2026: pack created. 29 Oct 2026: profiles, several graphs per node, disputes and views added. No run
recorded yet.
