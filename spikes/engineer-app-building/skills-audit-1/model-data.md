---
name: model-data
description: Load before creating or changing collections: records, keys, and what is stored or derived.
---

# Model the data

Collections can't be removed, so decide the shape first.

## Shape each collection

- **A copy of something external** (a ticket): one document per thing, keyed by its external ID.
- **Something that happened** (a move, a payment): append-only. Never edit it; derive current state from the log.
- **A setting one writer owns**: one document, updated in place.

If two writers can change one value at once, use a log. Concurrent updates to one field merge, and one is lost.

## Keys

Use a stable key with a unique index, so a repeated write is harmless. Use the source system's ID when there is one. Mark fields `@immutable` when they never change.

## Design for readers

List every reader first: each page section, trigger and agent.

- Index what you filter or sort on.
- A trigger's filter sees only the fields of the document that fired it. If one consumer should react, put the field that names it on the document.
- An agent's prompt is rendered from the firing document, so put the state it needs there.
- Queries return bounded pages. Plan for real row counts.

## Stored or derived

Derive what can be computed on read: an age, an overdue flag, a count. Store it only when reading is too slow. Store times as UTC `DateTime`. Use `@branchable` only for data that must sync between nodes.

## Check it

Write two or three realistic documents, read them back with your page's query, and confirm filters and sort order. Syntax: `schema help`.

Next: `connect-data` if the data comes from outside, otherwise `design-page`.
