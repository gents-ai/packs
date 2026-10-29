# claim_graph pack

An append-only claim graph kept as DefraDB documents, admitted only through a mechanical gate. A `Claim` is
one sourced statement with a verbatim quote from the narrative file it came from; an `Edge` relates two claims;
an `Entity` is a thing claims link to; a `Perspective` is an issuer (a person, a position, a critic). Writers
never create those rows directly: they create a `ClaimBatch` holding JSONL text, and the `claim_gate` plugin
checks the whole batch (the checks of a research graph's `validate.sh` and `quotes.sh`) and, in one transaction,
writes a `GateReceipt` and the admitted rows, or only a failing receipt. `claim_export` writes the graph back as
the same JSONL files, as a model tool or when an operator creates an `ExportRequest`. A **Claim graph expert** agent (`claim-sme`) answers questions from the graph with a claim
id on every statement and files the questions it cannot answer.

Status is never stored authoritatively. A claim's `status` field is its writer's advice; readers derive the
effective status from the edges pointing at it: `retracts`, then `supersedes`, then `contradicts`, else live.

```text
ClaimBatch (created) --claim-batch--> claim-gate (claim_gate plugin)
                                         |-- receipt      --> GateReceipt (always one)
                                         |-- claims       --> Claim       (on pass)
                                         |-- edges        --> Edge        (on pass)
                                         |-- entities     --> Entity      (on pass)
                                         `-- perspectives --> Perspective (on pass)

ExportRequest (created) --export-request--> claim-export (claim_export plugin)
                                              `-- receipt --> ExportReceipt (always one)
```

## Installation

```sh
gents pack install ./packs/gents/claim_graph --home <home> --inference-slot sme=<profile>
gents pack install gents/claim_graph --home <home> --inference-slot sme=<profile>   # once published
```

Building from a source checkout compiles both plugins to `wasm32-wasip1` (`gents pack build`), so it needs a
Rust toolchain with `rustup target add wasm32-wasip1`; a pack fetched pre-built skips the build.

## Bindings and prerequisites

- One inference slot, `sme`, bound to the `claim-sme` agent. Any capable profile. The gate needs no model.
- An allowed folder for the gate's file and quote checks: `source_root` must be an absolute path inside a
  folder added with `gents plugin dirs add <folder> --home <home>`. A callback has nobody to ask, so a
  `source_root` outside the allowed folders fails the call. For a research graph that lives at
  `<root>/graph/`, the source root is `<root>`.
- A writable allowed folder for export: `gents plugin dirs add <out> --home <home> --access read_write`.
- The question tool needs the perspective it files under and the entity `e:open-question` to exist already;
  importing a research graph brings both.

## Authority

| Who | May | Why |
| --- | --- | --- |
| `claim_gate` (callback) | query `Claim`, `Edge`, `Entity`, `Perspective`; read `source_root` | existence and uniqueness checks against the store; file and verbatim-quote checks. Its rows reach the store only as the callback's declared outputs, committed with the receipt |
| `claim_export` (tool and callback) | query the same four collections; read and write `out_dir` | pages every row and writes the JSONL tree; as the `claim-export` callback its only database write is the `ExportReceipt` |
| `claim-sme` (agent) | `query` over the four graph collections, `GateReceipt` and `ExportReceipt`; `file_question`; `claim_export` | retrieval and status derivation; its only write is a `ClaimBatch`, which still goes through the gate |

Nothing in the pack updates or deletes a graph row: every scalar field of every collection is `@immutable`
(DefraDB refuses the directive on the list fields `Claim.entities` and `Entity.aliases`), and a
wrong claim is answered with a new claim and a `retracts` or `supersedes` edge.

## Inputs and outputs

### ClaimBatch

| Field | Meaning |
| --- | --- |
| `batch_id` | unique correlation key, by convention `<section>-<yyyymmdd>-<n>`; every admitted row and the receipt carry it |
| `section` | the claims file stem the claims belong to (`datapack-flipper`); stored on each `Claim` |
| `claims`, `edges`, `entities`, `perspectives` | JSONL text, one JSON object per line, any subset |
| `source_root` | optional; when present, `from` files, cited `sources[].doc` files and verbatim quotes are checked under it; when absent they are skipped and the receipt counts them in `quotes_skipped` |
| `extractor`, `sample_size`, `failure_count`, `note`, `submitted_at` | provenance of the batch; not checked |

Line shapes (the research JSONL shapes, keys in this order):

| Line | Keys |
| --- | --- |
| claim | `id, text, quote, from, sources, entities, kind, by, asserted, status, confidence, note`, mapping claims also `fit, against, ref`, optional `valid_from, valid_to` |
| edge | `from, to, type, by, date, note` |
| entity | `id, type, name, aliases, note` |
| perspective | `id, name, kind, note`, optional `did` |

Stored rows rename the GraphQL-sensitive keys: a claim's `id` is `claim_id`, `from` is `from_ref`, `sources` is
the JSON text of the array; an edge is `from_claim`, `edge_type`, `to_claim` with
`edge_key = "<from_claim>|<edge_type>|<to_claim>"`; an entity is `entity_id`, `entity_type`; a perspective is
`perspective_id`. `Claim.entity_text` is the entity ids space-joined and `Entity.alias_text` the lowercased name
and aliases, so a `_like` filter finds them. `Claim.text` and `Entity.name` carry BM25 full-text indexes.

### Gate rules

Any error fails the whole batch.

1. Every line is a JSON object. Claim ids match `^c:[a-z0-9-]+:[0-9]{3}$`, are unique in the batch and absent
   from `Claim`.
2. A claim has `text, quote, from, sources, entities, kind, by, asserted, status, confidence`; `text` has at most
   40 words; `kind` is one of `fact number date rule mapping interpretation position quote question`; `status`
   one of `live superseded contested retracted`; `confidence` one of `sourced memory inferred`. A mapping claim
   has `fit` (`native partial gap convention`) and `against` (`defradb fefra`). At least one entity. A `sourced`
   claim cites a source with a `doc` or `code`; a `code` reference is `path[:line[-line]]`.
3. Every entity a claim names, and every `by`, exists in the batch or the store; entity and perspective ids in
   the batch are unique and new.
4. An edge has `from, to, type, by, date`; `type` is one of `supports contradicts supersedes retracts answers
   maps-to said-first depends-on`; both ends exist in the batch or in `Claim`; its `edge_key` is new.
5. With `source_root`: `from` (before `#`) and every cited `doc` exist under it, and every `quote` occurs
   verbatim in its `from` file, except claims filed as questions (`from` under `graph/claims/questions.jsonl`)
   and claims a `supersedes` or `retracts` edge (in the batch or the store) points at.

### GateReceipt

| Field | Meaning |
| --- | --- |
| `receipt_id` | `receipt-<batch_id>` |
| `verdict` | `pass` or `fail` |
| `errors` | JSON text of an array of `ERROR: ...` strings, at most 200 |
| `claims_admitted`, `edges_admitted`, `entities_admitted`, `perspectives_admitted` | rows written |
| `quotes_checked`, `quotes_skipped` | quote checks run and skipped |
| `gate_version`, `checked_at` | the plugin version and an ISO-8601 UTC time |

### Filing a question

The SME's `file_question` tool creates a `ClaimBatch` with `batch_id`, `section: "questions"` and `claims` holding
one line such as:

```json
{"id": "c:q:014", "text": "Which body issues Estonian e-seal certificates?", "quote": "Which body issues Estonian e-seal certificates?", "from": "graph/claims/questions.jsonl#questions", "sources": [], "entities": ["e:open-question"], "kind": "question", "by": "p:mike", "asserted": "2026-10-29", "status": "live", "confidence": "inferred", "note": "Searched e-seal and trust service; no issuer claim"}
```

### Export

`claim_export` with `{"out_dir": "<allowed folder>"}` writes `claims/<section>.jsonl`, `edges/edges.jsonl`,
`entities/entities.jsonl` and `perspectives/perspectives.jsonl`, each sorted by id, and returns the counts;
`{"dry_run": true}` returns the counts only. See `plugins/claim_export/TOOL.md`.

An operator exports without a model by creating an `ExportRequest`; the `claim-export` callback runs the plugin
and writes one `ExportReceipt`. `gents plugin run --bind-dir` binds its folder read-only, so it cannot write the
export; the callback is bound read-write through the allowed folder instead:

```sh
gents plugin dirs add /path/to/out --home <home> --access read_write
gents document create ExportRequest --home <home> --graphql http://127.0.0.1:8080/api/v0/graphql \
  --json '{"request_id": "export-20261029-1", "out_dir": "/path/to/out"}'
gents query find --home <home> --graphql http://127.0.0.1:8080/api/v0/graphql --collection ExportReceipt \
  --filter '{"request_id": {"_eq": "export-20261029-1"}}' --field ok --field error --field files_json
```

| `ExportRequest` field | Meaning |
| --- | --- |
| `request_id` | unique correlation key; the receipt carries it |
| `out_dir` | absolute path inside a folder allowed `read_write`; may be left out with `dry_run` |
| `sections` | JSON text of an array of claim section names; absent means every section |
| `dry_run` | count the rows and write nothing |
| `note`, `submitted_at` | provenance; not passed to the plugin |

| `ExportReceipt` field | Meaning |
| --- | --- |
| `receipt_id`, `request_id` | the receipt and its request |
| `ok`, `error` | whether the export finished, and the sentence it failed with |
| `claims_written`, `edges_written`, `entities_written`, `perspectives_written` | rows written (counted, with `dry_run`) |
| `sections_json`, `files_json` | JSON text of the sections exported and the files written |
| `dry_run`, `exported_at` | the mode and an ISO-8601 UTC time |

## Importing a research graph

`tools/import_jsonl.sh` is operator tooling, not a plugin. Against a running server:

```sh
gents plugin dirs add /path/to/research --home <home>
gents server --home <home> --http-port 8080 --p2p-transport none &
packs/gents/claim_graph/tools/import_jsonl.sh --source-root \
  /path/to/research/graph <home> http://127.0.0.1:8080/api/v0/graphql
```

It needs `jq` and `gents` (`GENTS` overrides the binary) and creates the batches with
`gents document create ClaimBatch`, waiting for each `GateReceipt` (`gents query find`) before the next:
perspectives and every `entities/*.jsonl` first, then each `claims/<section>.jsonl`, then the edges. A section
larger than `--chunk-bytes` (default 100000, under Linux's 128 KiB limit for one argument) is split into several
batches. A `supersedes` or `retracts` edge travels in the batch of the claim it points at, and the sections are
ordered so the superseding claim is admitted first, because the gate skips the quote check of a superseded claim
only when such an edge exists.

Running it again resumes. Each planned batch is a family of attempts: `<batch_id>`, then `<batch_id>-r2`,
`<batch_id>-r3` and so on. A family with a passing receipt is skipped. An attempt that has no receipt yet is
waited for, not resubmitted. A family whose attempts all failed in an earlier run is submitted again as
`<batch_id>-rN`, N being one more than its attempts so far (`retrying <batch_id> as <batch_id>-rN`). A failure
in the current run stops the import with the gate's errors; fix the input and run it again. `--suffix` starts
new families under new batch ids. `--dry-run` prints the plan. `--source-root=<dir>` names another source root; without the flag the file and quote checks are skipped.
An edge that repeats an earlier `(from, type, to)` is dropped with a warning naming both lines, since the gate
refuses an edge key twice; `--strict` stops instead.

## Completion and failure

A batch is complete when its `GateReceipt` exists. On `pass` its rows were committed in the same transaction as
the receipt; on `fail` nothing else was written and `errors` says why. A malformed host input (not a validation
failure) fails the callback call itself, and no receipt appears; the server log names the reason. A
`batch_id` is used once: correct a failed batch and submit it under a new id (the import script does this itself, as
`<batch_id>-rN`).

## Validation

```sh
gents pack check packs/gents/claim_graph
scripts/test-pack.sh packs/gents/claim_graph
```

The suite builds the pack, runs every plugin case (`plugins/*/tests/*.json`) and each plugin's `cargo test`,
the install case (`tests/install.json`: ten documents, two plugins, the `sme` slot), the configuration checks
in `tests/content.json`, and three runtime cases served from a throwaway repository: `tests/gate_runtime.json`
seeds a valid batch whose quote and cited document are in the repository and expects a passing receipt with one
`Claim`, `Entity` and `Perspective`; `tests/gate_runtime_fail.json` seeds a batch with an unknown kind, unknown
entity and perspective, a missing quote and a dangling edge and expects a failing receipt with nothing admitted;
`tests/export_runtime.json` seeds a dry-run `ExportRequest` and expects an `ExportReceipt` with `ok` and `dry_run`.

Research data the import tolerates: the research `edges/edges.jsonl` gives `c:exchange:084|supports|c:pitch:164`
twice (lines 67 and 181, with different `date` and `note`); the import keeps line 67 and warns about line 181.
Fix the source file to import it with `--strict`.

## Operational history

29 Oct 2026: pack created. No run recorded yet.
