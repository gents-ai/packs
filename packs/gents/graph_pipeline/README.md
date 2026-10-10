# Model-callable graph compiler evaluation

This is an evaluation pack for a small adapter over the existing Gents
automation runtime. It does not introduce another graph engine.

## Installation

```bash
gents pack install ./packs/gents/graph_pipeline --home <home>
gents pack install gents/graph_pipeline --home <home>   # once published to the registry
```

Assets packs take no `--inference-slot`. This installs these evaluation
assets locally; it does not create agent instances or launch inference. The
compiler examples below describe how to use the fixtures programmatically.

## Bindings and prerequisites

None: this is an assets pack with no inference slots and no runtime
configuration of its own.

## Authority

`compile_graph` accepts topology over configured capability revisions.
Each capability points at an existing Task document and declares its typed
ports. The model cannot author Task prompts, agents, tools, models, or
physical collections. The tool performs pure whole-graph validation first and,
only on success, writes the entry and edge EventTriggers in one transaction.
Selecting `compile_graph` in an agent's tools grants that agent the
publication capability.

```rust,ignore
let tool = CompileGraphTool::new(
    node,
    caller_identity,
    approved_existing_task_capabilities,
    CompilerPolicy::default(),
);

let agent = Agent::builder(provider).custom_tool(tool);
```

## Inputs and outputs

Input: a graph proposal (nodes, edges, entries, results, limits) submitted to
`compile_graph`. Output: on acceptance, an entry document and the graph's edge
EventTriggers written in one transaction; execution remains separate. After
normal runtime reconciliation, an existing bounded write tool creates an entry
document and the ordinary trigger/task engine runs the graph.

`eval_cases.json` is compiled as part of the gents unit suite
(`crates/gents/src/graph_pipeline/tools.rs`). It covers valid single- and
multi-stage proposals plus repair cases for invented or stale capabilities,
missing inputs, schema mismatch, cycles, and structural bounds.

## Completion and failure

Expected diagnostic codes are subsets so cases remain stable when the compiler
adds another useful diagnostic. For model evaluation, record proposal
acceptance, diagnostic codes per repair turn, number of repair turns, final
digest, publication errors, reconciliation latency, and whether the separately
written entry document drives the expected existing Tasks. Stage-output
quality is a separate evaluation.

## Validation

```bash
gents pack check ./packs/gents/graph_pipeline
gents pack test ./packs/gents/graph_pipeline
make test-graph_pipeline
```

`tests/install.json` pins the files an install materializes and asserts a
remove releases them.

## Operational history

None recorded yet.
