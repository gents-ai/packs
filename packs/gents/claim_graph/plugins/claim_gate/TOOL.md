# `claim_gate`

The mechanical gate of the claim graph. It runs as the `claim-gate` callback
when a `ClaimBatch` is created: it checks the batch against its graph's
profile and the research gate's rules and, when every rule holds, admits all
of its lines as `Claim`, `Edge`, `Entity` and `Perspective` rows of that
graph (and, when the batch carries one, its profile as the graph's next
`ClaimGraph` version). The host writes the receipt and the rows in one
transaction, so a batch is admitted whole or not at all. Nobody writes those
collections any other way.

## Input

The `ClaimBatch` fields: `batch_id` (required; the host copies it into every
row and the receipt), `graph_id` (the graph the batch belongs to; ids are
unique per graph), `profile`, `section` (the claims file stem; required when
the batch has claims), `source_root`, and `claims`, `edges`, `entities`,
`perspectives`, each JSONL text in the research shapes (one object per line;
blank lines are skipped). `extractor`, `sample_size`, `failure_count`,
`note`, `submitted_at` and any field the host adds are accepted and not read.

`profile` is the JSON text of a graph's vocabulary: the `ClaimGraph` fields
except `graph_id`, `version`, `batch_id` and `created_at`, which the gate
assigns. `kinds`, `confidence_classes` and `edge_types` are each a non-empty
list of distinct strings. `mapping_targets`, `mapping_fits`,
`review_verdicts` and `resolver_perspectives` are lists of distinct strings
that may be empty or absent (read as empty): then every mapping claim fails
`mapping without valid fit`/`against`, every review fails `review without
valid verdict` and every `resolves` edge fails the resolver rule, and the
admitted `ClaimGraph` row omits the empty list;
`claim_id_pattern` is a regular expression a claim id must match (anchor it);
`text_max_words` is 1 to 400; `display_name`, `question_entity`,
`default_asker`, `questions_from_prefix` and `note` are optional text. Every
resolver must be a perspective in the batch or the graph. A graph's first
batch must carry a profile; a later batch may carry one to admit a new
version, or omit it to be checked against the latest stored version.

`source_root` is the narrative source root inside a folder the operator
allowed (`gents plugin dirs add`). The host binds it read-only. With it, the
gate checks that each claim's `from` file (before `#`) and every non-null
`sources[].doc` exist under it, and that each quote occurs verbatim in its
`from` file. Without it those checks are skipped.

`gate_now`, which the binding never sends, fixes `checked_at` for
reproducible plugin cases.

## Rules

Any broken rule fails the whole batch. The wording is the research gate's
(`validate.sh`, `quotes.sh`, `edge.sh`) where the rule is its rule.

- The batch names a `graph_id`. Its profile is the inline one, or the
  graph's latest stored version; with neither: `ERROR: unknown graph <id>:
  the first batch must carry a profile`.
- Every line is a JSON object (`ERROR: claims:3 invalid JSON`).
- A claim id matches the profile's `claim_id_pattern`. Ids of claims,
  entities and perspectives, and the edge key `<from>|<type>|<to>`, are
  unique in the batch and new to the graph.
- A claim has `text, quote, from, sources, entities, kind, by, asserted,
  status, confidence`; `text` has at most `text_max_words` space-separated
  words; `kind` is in `kinds`, `confidence` in `confidence_classes`, `status`
  is `live`, `superseded`, `contested` or `retracted`; a `mapping` has `fit`
  in `mapping_fits` and `against` in `mapping_targets`; at least one entity;
  a `sourced` claim cites a `doc` or a `code` reference; a `code` reference
  is `path[:line[-line]]`. A claim carries no field outside the schema, and
  text fields are strings.
- Every entity a claim names, and every `by`, is in the batch or the graph.
- An edge has `from, to, type, by, date`, a type in `edge_types`, and both
  ends in the batch or the graph.
- Disputes, when the profile lists the kinds and edge types:
  - a `challenge` or `review` claim comes with exactly one `challenges` or
    `reviews` edge from it in the same batch (`ERROR: <id> is a challenge
    without its challenges edge`);
  - a `challenges` or `reviews` edge leaves a claim of that kind, in the
    batch or the graph;
  - a `review` has a `verdict` in `review_verdicts` (`ERROR: <id> review
    without valid verdict`);
  - a `resolves` edge's `by` is in `resolver_perspectives` (`ERROR: edge
    <from>-><to> resolves by <by>, not a resolver`).
- With `source_root`: files exist, and quotes are found as `grep -F` finds
  them (each line of a multi-line quote is its own pattern). Not
  quote-checked: a `question`, `challenge` or `review` whose `quote` equals
  its `text`; a claim whose `from` starts with the profile's
  `questions_from_prefix`; a claim that a `supersedes` or `retracts` edge
  points at, in the batch or the graph.

## Rounds

The first round checks everything that needs no store (with an inline
profile, its vocabulary too). A batch that breaks one of those rules, or
names a missing file, fails at once (with its quote errors too). Otherwise
the plugin asks the store, through `data_calls`, for the graph's latest
`ClaimGraph` version and which of the ids the batch holds or names already
exist in the graph (`_and` of `graph_id` and an `_in` of 256 ids, 16 queries
per round, its cursor in `state`), and the last round resolves the profile
and applies every rule and the quotes. A batch needing more than 960 queries
fails and asks to be split.

## Output

An object keyed by port. `receipt` is always present: a `GateReceipt` with
`receipt_id` (`receipt-<batch_id>`), `graph_id`, `verdict` (`pass` or
`fail`), `errors` (the JSON text of an array of `ERROR: ...` strings, at most
200, the last saying how many more were cut), `claims_admitted`,
`edges_admitted`, `entities_admitted`, `perspectives_admitted` (0 on a fail),
`quotes_checked`, `quotes_skipped` (exempt claims, and every claim when there
is no `source_root`), `profile_version`, `gate_version` and `checked_at`
(UTC). `profile_version` is the version the batch was checked against: the
stored one, or for an inline profile the version it is admitted as (one more
than the graph's highest). It is absent when no profile was resolved: a
first-round failure, a store refusal, or an unknown graph.

On a pass, `claims`, `edges`, `entities` and `perspectives` carry one row per
line, each with the batch's `graph_id`. A claim row maps `id` to `claim_id`,
`from` to `from_ref`, stores `sources` as its JSON text, keeps `verdict`,
adds `section` and `entity_text` (the entity ids space-joined with a space at
each end, for `_like: "% e:fcc %"`); an entity row maps `id`/`type` to
`entity_id`/`entity_type` and adds `alias_text` (name and aliases, lowercased,
joined the same way); an edge row maps `from`/`to`/`type` to
`from_claim`/`to_claim`/`edge_type` and adds `edge_key`. A field absent from
the line is absent from the row, and so is an empty `aliases` list. When the
batch carried a profile, `profile` is its `ClaimGraph` row: the profile's
fields plus `graph_id`, `version` and `created_at` (`checked_at`). On a fail
only the receipt is written.

A failed rule is a `fail` receipt, never a failed call. The call itself fails
(one sentence on stderr) only for malformed host input: no `batch_id`, a
batch field that is not text, no `data_calls` offered, or a `state` or
`data_results` the plugin did not ask for. A store query the host refuses is
a `fail` receipt naming it.
