# `claim_export`

Writes one graph back out as the research repository's JSONL tree, so the
research tools (`validate.sh`, `check.sh`) and diffs work on it. It reads the
graph's `Claim`, `Edge`, `Entity`, `Perspective` and `ClaimGraph` rows (every
query filters on `graph_id`) and writes, under the folder you name:

- `claims/<section>.jsonl`, one file per claim section
- `edges/edges.jsonl`
- `entities/entities.jsonl`
- `perspectives/perspectives.jsonl`
- `claim_graph.json`: the graph's profile versions, a JSON array ordered by
  `version`, one compact object per line holding every ClaimGraph field in
  schema order (a stored null stays null)

Lines are compact JSON in the research key order, sorted by id (edges by
`from|type|to`). Existing files of those names are replaced whole; other files
in the folder are left alone, including section files of sections no longer in
the store.

## Input

- `graph_id` (required): the graph to export.
- `out_dir` (required unless `dry_run`): the folder to write into. It must be a
  folder the operator has allowed for writing.
- `sections`: export only the claims of these sections (file stems). Edges,
  entities and perspectives are always exported in full.
- `dry_run`: count and list the files without writing anything.

## Output

`{"ok": true, "graph_id": "...", "dry_run": false, "written": {"claims": n,
"edges": n, "entities": n, "perspectives": n, "claim_graph": n}, "sections":
{"<section>": n}, "files": ["claims/<section>.jsonl", ..., "claim_graph.json"]}`. With `dry_run`, `written` holds the counts
that would be written and `files` the paths.

Failure: `{"ok": false, "error": "...", "retryable": false}`. Nothing is
written when a read fails; a write failure can leave earlier files of the same
call replaced.

Status is never exported as derived state: `status` is the writer's advisory
value from the claim row. Empty optional fields are left out (`fit`, `against`,
`ref`, `valid_from`, `valid_to`, a perspective's `did`); `note` is always
written.

## Authority and limits

Read-only queries on `Claim`, `Edge`, `Entity`, `Perspective` and
`ClaimGraph`; writes only inside `out_dir`.
No network, environment or commands. About 100000 rows per collection is the
most the host lets one call page.

## Callback mode

When the input carries `request_id`, the plugin runs as the callback of an
`ExportRequest` document and answers with one flat
`ExportReceipt` row (the output object itself, no wrapper key): `receipt_id`
(`export-<request_id>`), `graph_id`, `ok`, `error`,
`claims_written`, `edges_written`, `entities_written`, `perspectives_written`,
`sections_json`, `files_json` (JSON text), `dry_run`, `exported_at` (UTC
ISO-8601). The profile count has no receipt field; `files_json` lists
`claim_graph.json`. It accepts `graph_id` (required), `out_dir`, `sections` (a list or its JSON text; blank
means all), `dry_run` and the host fields, and ignores other fields. Every
failure, including a missing `out_dir` without `dry_run`, is a receipt with
`ok: false`, never a non-zero exit. `out_dir` must be a folder the callback is
allowed to write; a read-only bind (`gents plugin run --bind-dir`) fails the
write into a receipt, so use `dry_run` there.
