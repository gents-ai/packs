---
name: write-plugin
description: Load when compiled rules must be exact and queries or tools can't express them. Contract, data, tests.
---

# Write a plugin

Use one only when a query or tool can't do the job. Scaffold from the `plugin-tool` template; JavaScript is the shortest path: `module.exports = (input) => ({ ok: true })`. Keep it pure: input in, answer out. Take randomness from a seed in the input, so results can be replayed.

## Contract

One JSON value in, one out. An error the caller can act on is a normal output, such as `{ "ok": false, "refusal": { "code": "...", "message": "..." } }`.

## Reading documents

Declare access in the plugin's manifest entry. Without it, the plugin has no data access:

```json
"data": { "collections": ["GameEvent"], "ops": ["query", "insert"], "stamped": {} }
```

To read, return this; a bare array is refused:

```json
{ "data_calls": { "requests": [ { "id": "log", "query": { "collection": "GameEvent", "filter": { "game_id": { "_eq": "g1" } }, "order": { "seq": "ASC" }, "limit": 256, "fields": ["seq", "kind"] } } ] } }
```
 The host runs the requests and calls the plugin again with the original input. Results arrive at `input.data_results.<request id>.result.rows`. A query returns at most 256 rows, so page with `offset`. A call takes at most 64 rounds.

## Writing documents

Put `writes` on the final output: `"writes": [ { "op": "insert", "collection": "GameEvent", "fields": { "seq": 12 } } ]`. At most 100 per call, committed together. The plugin's rules decide what enters the log.

## Test before you install

- Put `input` and `expect` pairs in `plugins/<name>/tests/*.json`, one per rule, including the refusals that must happen.
- Run an installed plugin with `gents plugin run <name> --input '<json>'`.
- Build and test through `config` (`pack build`, `pack test`). If you can't run the tests, report the plugin as not tested.

Then return to `plan-work`.
