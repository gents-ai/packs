# rhai_workflows_live

Live model runs of the `rhai_workflows` pack. A scenario pack for `make live`; not published.

## Configuration

Depends on the `rhai_workflows` pack. It ships the caller agent `wf-live-caller`, the AgentTarget `wf-live-worker` (read-only files, no shell), the `workflows/` fixtures with their `index.json`, and `fixtures/notes.txt` for the tool root.
`GENTS_LIVE_ROOT` is the tool root holding a copy of `fixtures/`; `GENTS_LIVE_ENDPOINT` and `GENTS_LIVE_MODEL` select the OpenAI-compatible model, so a second backend is a second `LIVE_ENDPOINT` and `LIVE_MODEL`.
`experiment.json` carries a literal `await_timeout_secs` of 1800, so `LIVE_AWAIT_TIMEOUT_SECS` does not apply to this scenario.
The `wf-research` and `wf-read-only-web` personas ship in the `rhai_workflows_web` pack, which depends on `rhai_workflows` and `gents/web`; the matrix installs it only in the homes that run `wf-research` (row 13 and the corpus legs).

## Wiring

`make live` installs neither pack as operator and wires nothing. The matrix script `matrix.sh` builds one home per row and backend and does the following in it, with the server stopped:

1. Install the engine with its raised limits, then the scenario pack without them:
   `gents pack install packs/gents/rhai_workflows --home "$H" --grant-authority`, then `gents pack install scenarios/rhai_workflows_live --home "$H"`.
2. Wire the caller with the row's packs only:
   `gents pack wire gents/workflow --agent wf-live-caller --targets-from gents/rhai_workflows_live --home "$H"`. Row 12 passes no `--targets-from`, and row 13 passes `--targets-from gents/rhai_workflows_web`. Never hand-edit `trusted_packs`.
3. Narrow the wired block to the row's targets (`wf-live-worker`, or the persona ids rows 12 and 13 name); the step only removes targets.
4. Start the server from the work folder with `--tool-ceiling readonly --tool-root <work folder>`. Without them the server runs at `meta-only` with no tool root, the workers lose their file tools, and a job's children get an empty working folder and are refused at admission.

## Full matrix

`matrix.sh` runs acceptance rows 1-22 (`rows/r<N>.json`) on two backends, one home per row and backend.
It reads `LIVE_ENDPOINT_1`/`LIVE_MODEL_1` and `LIVE_ENDPOINT_2`/`LIVE_MODEL_2`, and the gents binary from `GENTS` or `RW_GENTS_BIN`:

```sh
export RW_GENTS_BIN=/path/to/gents
LIVE_ENDPOINT_1=http://workstation-1:8000/v1 LIVE_MODEL_1=<model> \
LIVE_ENDPOINT_2=http://workstation-2:8000/v1 LIVE_MODEL_2=<model> \
  scenarios/rhai_workflows_live/matrix.sh [--rows 12,13] [--backend 1|2] [--dry-run]
```

`--list` prints the rows; `--dry-run` prints each home's installs, wiring and narrowing without running anything.
Evidence goes to `<run dir>/<backend>/row-<N>/`.
In those homes `matrix.sh` installs `packs/gents/rhai_workflows_web` as operator with `--grant-authority` and every inference slot bound, right after the engine; that also installs `gents/web` and grants all of its plugins. No other row has `gents/web`.

Per-row home: `wire_from` in the row file names the packs passed as `--targets-from`, and `targets` is what the wired block is narrowed to.
Rows 14-21 wire `gents/rhai_workflows_live` and keep `wf-live-worker`. Row 14 also sets `job.policy.dynamic` to `run`, because a kept workflow runs as dynamic, and ends by running the kept name, a script with a real `agent()` call, to a completed job with one child on `wf-live-worker`.
Row 12 wires nothing and keeps `wf-read-only` and `wf-read-write`, with `job.policy.dynamic` set to `run` before the server starts, so the inline script's `wf-read-write` request is refused by `dynamic_targets`.
Row 22 wires nothing and keeps `wf-read-only`, with `job.policy.dynamic` set to `run`: the model runs an inline script that spawns one `wf-read-only` agent over `notes.txt`. The row asserts a completed job with one completed child on `wf-read-only` (`AgentRequest.agent_id`), `pin.origin` `dynamic` in `gents job show`, and the agent's output marker in the completion notice.
Row 13 wires `gents/rhai_workflows_web` and keeps `wf-read-only` and `wf-research`: the scenario pack is installed but not trusted, so its fixture runs as dynamic and its first spawn on `wf-research` is refused.
No row wires `gents/rhai_workflows`; row 17 only prints its reference (the gents end-to-end test that refuses composing a job tool with `job_tool_not_composable`), because no packs plugin composes.

Client legs on every run: `gents job list`, `show` and `watch`, and `gents query find` over `AgentToolCall.job_progress`, which carries `pause_reason`.
`stop` and `pause` act on the local runtime only, so they run with `--home` and the server stopped (row 7). With no server the stop only latches each live child's interrupt, so row 7 checks the latch and then restarts the node to see the children end.
The Grok pager and desktop checks cannot run headless, so each run prints them as a manual checklist with its address.

Corpus legs: with `GROK_CORPUS_DIR` set to a folder of `*.rhai` files, `matrix.sh` runs each file found there as a local path on `wf-research`, in row 13's home.
A Grok-only script, or one whose spawn the home refuses, is reported as such and does not fail the matrix.
`--rows corpus` runs only these legs.

## Usage

```sh
make live LIVE_PACK=rhai_workflows LIVE_ENDPOINT=http://workstation-1:8000/v1
```
