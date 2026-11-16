# `claim_views`

Derives a view of one claim graph and returns it as Markdown in a
`ViewReceipt`. It runs as the callback of a `ViewRequest` document: create the
request, then read the receipt whose `request_id` matches.

## Views

- `status`: how many claims have each effective status, then every claim that
  is not live with the edge that decided its status and that edge's `by`.
  `rows` is the number of claims that are not live.
- `disputes`: every open challenge: the target claim and its text (first 120
  characters), the challenger perspective, the challenge claim and its text,
  the days since the challenge was asserted, and the reviews of the target
  with their verdicts. `rows` is the number of open challenges.
- `reputation`: one row per perspective (every `Perspective` of the graph and
  any other claim author): claims asserted; `sourced`, `memory` and `inferred`
  counts; claims later retracted; claims later superseded; claims an `answers`
  edge points at; challenges raised and how many of them an `answers` or
  `resolves` edge closed; challenges received; reviews given by verdict.
  `rows` is the number of perspectives. It is counts only: a weighting or
  score belongs to whoever reads the table.

## Effective status

For a claim X: `retracted` if any `retracts` edge points at X; else
`superseded` if any `supersedes` edge points at X; else `contested` if a
`contradicts` edge touches X and no `supersedes` edge (either way) joins that
edge's two claims; else `challenged` if a claim of kind `challenge` has a
`challenges` edge to X and no `answers` or `resolves` edge points at that
challenge; else `live`. A `resolves` edge pointing at a review marks it
adjudicated and changes no status; reviews never change status. When several
edges qualify, the deciding edge is the first by edge key.

## Input

The `ViewRequest` fields: `request_id` (required), `graph_id` (required, a
name of letters, digits, `-`, `_` and `.`), `view` (`status`, `disputes` or
`reputation`), `out_dir`, `dry_run`; the host adds `data_calls`,
`data_results`, `state` and `out_dir_original`. Other fields are ignored.
`view_now` (an ISO-8601 date or UTC timestamp) replaces the clock for
`generated_at` and the days open, so a test renders the same bytes every time.

With `out_dir` (a folder the callback may write) and without `dry_run`, the
Markdown is also written to `<out_dir>/<graph_id>/<view>.md`, replacing the
file whole. A blank `out_dir` means none.

## Output

One flat `ViewReceipt` row (no wrapper key): `receipt_id`
(`view-<request_id>`), `graph_id`, `view`, `ok`, `error`, `rows`, `markdown`,
`file` (the written path relative to `out_dir`, or `""`), `generated_at`.
`markdown` holds the whole view up to 60000 characters; a longer view is cut at
a line end and ends with a line saying it was truncated and where the whole
view is.

Every failure, a bad request field, a missing data grant, a failed query or a
failed write, is a receipt with `ok: false`, the reason in `error`, `rows` 0
and empty `markdown`, never a non-zero exit.

## Authority and limits

Read-only queries on the graph's `Claim` and `Edge` rows (and `Perspective`
for `reputation`), each filtered on `graph_id`; writes only inside `out_dir`.
No network, environment or commands. About 100000 rows per collection is the
most one call can page.
