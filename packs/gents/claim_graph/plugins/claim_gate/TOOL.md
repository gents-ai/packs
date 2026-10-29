# `claim_gate`

The mechanical gate of the claim graph. It runs as the `claim-gate` callback
when a `ClaimBatch` is created: it checks the batch by the research gate's
rules and, when every rule holds, admits all of its lines as `Claim`, `Edge`,
`Entity` and `Perspective` rows. The host writes the receipt and the rows in
one transaction, so a batch is admitted whole or not at all. Nobody writes
those collections any other way.

## Input

The `ClaimBatch` fields: `batch_id` (required; the host copies it into every
row and the receipt), `section` (the claims file stem; required when the batch
has claims), `source_root`, and `claims`, `edges`, `entities`, `perspectives`,
each JSONL text in the research shapes (one object per line; blank lines are
skipped). `extractor`, `sample_size`, `failure_count`, `note`, `submitted_at`
and any field the host adds are accepted and not read.

`source_root` is the narrative source root inside a folder the operator
allowed (`gents plugin dirs add`). The host binds it read-only. With it, the
gate checks that each claim's `from` file (before `#`) and every non-null
`sources[].doc` exist under it, and that each quote occurs verbatim in its
`from` file. Without it those checks are skipped.

`gate_now`, which the binding never sends, fixes `checked_at` for
reproducible plugin cases.

## Rules

Any broken rule fails the whole batch. The wording is the research gate's
(`validate.sh`, `quotes.sh`, `edge.sh`).

- Every line is a JSON object (`ERROR: claims:3 invalid JSON`).
- A claim id matches `c:<section>:<nnn>`. Ids of claims, entities and
  perspectives, and the edge key `<from>|<type>|<to>`, are unique in the batch
  and new to the store.
- A claim has `text, quote, from, sources, entities, kind, by, asserted,
  status, confidence`; `text` is at most 40 space-separated words; `kind`,
  `status` and `confidence` are from the schema's lists; a mapping has `fit`
  and `against`; at least one entity; a `sourced` claim cites a `doc` or a
  `code` reference; a `code` reference is `path[:line[-line]]`. A claim carries
  no field outside the schema, and text fields are strings.
- Every entity a claim names, and every `by`, is in the batch or the store.
- An edge has `from, to, type, by, date`, a type from the schema's list, and
  both ends in the batch or the store.
- With `source_root`: files exist, and quotes are found as
  `grep -F` finds them (each line of a multi-line quote is its own pattern).
  A question (`from` is `graph/claims/questions.jsonl`) and a claim that a
  `supersedes` or `retracts` edge points at, in the batch or the store, are
  not quote-checked.

## Rounds

The first round checks everything that needs no store. A batch that breaks
one of those rules, or names a missing file, fails at once (with its quote
errors too). Otherwise the plugin asks the store, through `data_calls`,
which of the ids the batch holds or names already exist (`_in` queries of 256
ids, 16 per round, its cursor in `state`), and the last round applies the
store's answers and the quotes. A batch needing more than 960 queries fails
and asks to be split.

## Output

An object keyed by port. `receipt` is always present: a `GateReceipt` with
`receipt_id` (`receipt-<batch_id>`), `verdict` (`pass` or `fail`), `errors`
(the JSON text of an array of `ERROR: ...` strings, at most 200, the last
saying how many more were cut), `claims_admitted`, `edges_admitted`,
`entities_admitted`, `perspectives_admitted` (0 on a fail), `quotes_checked`,
`quotes_skipped` (questions, superseded or retracted claims, and every claim
when there is no `source_root`), `gate_version` and `checked_at` (UTC).

On a pass, `claims`, `edges`, `entities` and `perspectives` carry one row per
line. A claim row maps `id` to `claim_id`, `from` to `from_ref`, stores
`sources` as its JSON text, adds `section` and `entity_text` (the entity ids
space-joined with a space at each end, for `_like: "% e:fcc %"`); an entity
row maps `id`/`type` to `entity_id`/`entity_type` and adds `alias_text` (name
and aliases, lowercased, joined the same way); an edge row maps
`from`/`to`/`type` to `from_claim`/`to_claim`/`edge_type` and adds
`edge_key`. A field absent from the line is absent from the row, and so is an
empty `aliases` list. On a fail only the receipt is written.

A failed rule is a `fail` receipt, never a failed call. The call itself fails
(one sentence on stderr) only for malformed host input: no `batch_id`, a
batch field that is not text, no `data_calls` offered, or a `state` or
`data_results` the plugin did not ask for. A store query the host refuses is
a `fail` receipt naming it.
