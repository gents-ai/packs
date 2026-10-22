# review_retarget_live

Live model run of a Goal-backed Task that waits on the `review-changes` workflow job and records a ProbeReport. A scenario pack for `make live`; not published.

## Fixtures

`fixtures/make_repo.sh` builds `probe_repo`, a repository with a base commit and a head commit, from `fixtures/base`. The review base must be a commit, so a one-commit repository cannot be reviewed.

## What the run asserts

The Task `retarget-probe-task` (`goal_objective_template`, `emit_outcome: true`) follows the five-step protocol of code_review's `Use from a Task` and ends with the terminal tool `write_probe_report` (ProbeReport: `head_sha`, `confirmed_count`; schema in `schemas/`). `experiment.json` gates the run on the `workflow` tool result, the ProbeReport document and the background completion counts. Its `manual_graphql_readback_not_enforced` key lists four conditions that the scenario runner does not know and `make live` does not enforce; read them back over GraphQL after the run and record the result as evidence: the `workflow` AgentToolCall is terminal at T_job; a FireOutcome with `created_at >= T_job` is complete; the ProbeReport was written after T_job and matches the job result; at most 3 Goal continuation requests ran while the job ran.

## Configuration

Depends on `code_review` and `rhai_workflows`. `pack_config.json` grants the `gents/workflow` entry the `job` block and the `agents` block that target the four code-review-* agents. `GENTS_LIVE_ROOT` holds a copy of `fixtures/`; `GENTS_LIVE_ENDPOINT` and `GENTS_LIVE_MODEL` select the model.

## Usage

```sh
make live LIVE_PACK=review_retarget LIVE_PACK_DIR=<worktree>/packs/gents/code_review LIVE_WITH_PACKS=rhai_workflows LIVE_ENDPOINT=<url> LIVE_MODEL=<model>
```

`LIVE_PACK_DIR` is overridden because it defaults to `packs/gents/review_retarget`, which does not exist; the pack under test is `code_review`.

The live run is the evidence for the retarget; it is not part of the pack gate.
