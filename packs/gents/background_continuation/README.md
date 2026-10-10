# Background continuation pack

This pack proves the operator-visible path behind #1114 and #1116:

Runtime configuration is authored once in `pack_config.json`. The distribution
`manifest.json` points to that canonical bundle and lists it with the schema and
prompt sidecars needed to install the pack; there are no per-collection JSON
document fragments.

```text
create BackgroundContinuationJob
              │
              ▼
      parent task completes
         │             │
         ▼             ▼
  background child  background child
         │             │
         └──── durable completion notifications ────┐
                                                    ▼
                                      coalesced continuation wake
                                                    │
                                                    ▼
                                     exact snapshot acknowledged
```

## Installation

```bash
gents pack install ./packs/gents/background_continuation --home <home> \
  --inference-slot coordinator=<profile_id> --inference-slot worker=<profile_id>
gents pack install gents/background_continuation --home <home> \
  --inference-slot coordinator=<profile_id> --inference-slot worker=<profile_id>   # once published to the registry
```

## Bindings and prerequisites

The pack declares two inference slots: `coordinator`, bound to the
`background-parent` agent (owns the durable continuation and synthesizes
worker results), and `worker`, bound to `background-worker` (runs bounded
background investigations). Bind both at install time as shown above.

## Authority

`background-parent-tools` grants background agent-target orchestration only: it
may target the `background-parent-tools:worker` agent target
(`background-worker` agent, "Returns one concise analysis result") and
nothing else. `background-worker-tools` grants no tools at all.

## Inputs and outputs

Input: create a `BackgroundContinuationJob` document, which fires the
`background-parent` trigger. Output: the parent task delegates to two
background `worker` agent invocations and, once both complete, a coalesced
continuation wake with an exactly-acknowledged snapshot; no other document is
written by this pack.

## Completion and failure

A complete run reaches two completed depth-positive child requests, at least
one completed canonical background-completion wake, at least two acknowledged
notification keys, and zero pending or stranded notifications (see Validation
for the scenario runner that checks this).

## Validation

Run it with a fresh home:

```bash
gents pack scenario run background_continuation
```

The runner requires two completed depth-positive child requests, at least one
completed canonical background-completion wake, at least two acknowledged
notification keys, and zero pending or stranded notifications. It also exports
the parent and wake timelines plus ATIF, OpenAI Codex, LangGraph, and
multi-agent projections under `runs/<job_id>/projections/`.

This provider-backed pack validates the successful end-to-end and successor-
epoch paths. Crash cuts are kept deterministic: the Lean R6 contract emits the
before-claim, during-inference, after-response-persistence, and acknowledgement
restart cases; the queue tests exercise restart recovery, persisted-response
repair, failed-wake redrive, and exact successor acknowledgement against the
real store.

```bash
gents pack check ./packs/gents/background_continuation
gents pack test ./packs/gents/background_continuation
make test-background_continuation
```

`tests/install.json` pins the `coordinator`, `worker` slots and the 10
documents an install creates, reinstalls without change and removes.

## Operational history

None recorded yet beyond the scenario runner's own pass/fail gate described
above.

## Declared topology

Document-trigger edges; task writes and host callbacks are described above.

<!-- pack-topology:start -->
```mermaid
flowchart LR
    n0["BackgroundContinuationJob"]
    n1["background-parent-task"]
    n0 -->|"background-parent"| n1
```
<!-- pack-topology:end -->
