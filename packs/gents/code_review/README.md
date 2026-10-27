# Code review

Four-stage code review personas (recon, scan, verify, triage) and the
`review_changes` job plugin that drives them, installed as agent targets.

The pack is a `documents` pack. It ships one Rust plugin
(`plugins/review_changes`, built to `plugins/review_changes.afb`), four
personas and one agent target per persona (`code-review-recon`,
`code-review-scan`, `code-review-verify`, `code-review-triage`). The plugin
is the workflow: each job step reads the host's view of the run (which child
slots exist and how they ended) and answers with the next spawn, a
correction, or the result. There is no script and no engine dependency.

## Operator steps

1. Install this pack as the operator, with `--grant-authority` for the
   plugin's declared limits:

   ```sh
   gents pack install gents/code_review --home ./.gents --node-did "$REVIEW_NODE_DID" \
     --grant-authority \
     --inference-slot coordinator=claude-coordinator \
     --inference-slot worker=glm-worker \
     --inference-slot verifier=grok-verifier
   ```

2. Wire the plugin to the agent that may start reviews, taking the four
   targets from this pack:

   ```sh
   gents pack wire gents/review_changes --agent <A> --targets-from gents/code_review
   ```

   This adds the `review_changes` tool to the agent's Tools with a `job`
   block naming the four target ids, adds them to `AgentTools.target_ids`,
   and adds `gents/code_review` to `job.policy.trusted_packs`. The pack's
   `Tools/review-changes-caller` document is the reviewed template the job
   block is copied from.
3. Bind the three inference slots (shown in step 1). `coordinator` serves
   recon and triage, `worker` serves the parallel scans and `verifier`
   serves adversarial verification. Use the same home and node for
   installation and execution.

## Authority

An agent may run a persona only when the target is in both the operator's
`job.agent_targets` and the agent's `AgentTools.target_ids`. That
intersection replaces the 1.x `allowed_callers` lists and keeps the same
guarantee: the operator decides who may run the review.

The personas hold the least tooling they need:

- `review-recon` and `review-scan` have read-only bash and no file tools.
  Scan output is capped at 64,000 characters per command.
- `review-verify` has read-only bash only, with the network disabled.
- `review-triage` has no tools.
- None of them has the `sessions` history tool: a child answers from its own
  work and never by reading transcripts, which in live runs sent a verifier and
  a triage agent into a read loop until the model's context ran out.

Reviewed source stays read-only throughout, and review output is evidence,
not permission to merge.

## Origin

The tool is called with `args` and `agent_budget` only: no `argv`, `script`
or `path`. The host classifies such a call `installed` from the plugin
artifact's own provenance (the pack operator-installed at the pinned digest),
so the pack ships no `workflows` mount or marker file; the pin's digest is the
sha256 of the embedded `plugins/review_changes/source/workflow.json`. With
the pack installed and trusted by step 2, every run is `installed` under the
default `policy.dynamic: check`.

## Args and defaults

The args, their defaults and limits are in `plugins/review_changes/TOOL.md`
(Input) and the manifest `input_schema`.

The plugin needs `agent_budget`, and the job block's `max_spawns`, of at least
22 (recon, four scans, up to 16 verifiers and triage). With less it pauses with
`budget_exceeded` before any agent runs; call `review_changes` again with a
higher `agent_budget`, or raise `max_spawns`.
Panels wider than the job block's `max_live` start in windows as children
finish.

## Result contract

On completion the job's result is:

- `status`: `complete` or `blocked`
- `base_sha`, `head_sha`, `diff_id`: the pinned change and its fingerprint
- `counts`: `candidate`, `confirmed`, `refuted`, `unverified`, `high_priority`
- `findings`: the confirmed findings
- `report`: the triage report, including `merge_recommendation`
- `limitations`: every blocked lens, unverified candidate or other gap
- `path`: the scratch report, see below

Read the full value with `gents job show <address>` or the job result
readers; the background completion notification is capped at 4,000 B and is
not the result.

A finding is confirmed only when its verifier returned `confirmed` with
confidence of at least 80 and non-empty evidence, and the verifier resolved
the same trees as recon. Zero findings is distinct from a blocked review.

Each child must end its reply with one `json` fence (a bare JSON object is
accepted too) that conforms to its output schema. A reply that does not is
corrected at most twice through a follow-up message, then the slot is given
up as a limitation. The plugin carries only failed judgements and correction
counts between steps in the host's opaque job state (`state` on its effects,
echoed on the next step's input), keeping model text out of bounded state.
A successful child's text is not carried: each step that judges it reads it
again, with every other completed child's text, in one `job_calls` round.

## Scratch report

The report is written to `<cwd>/.gents/workflows/runs/<tool_call_id>/review.md`,
where `cwd` is the reviewed repository or an ancestor of it. Repositories that
do not already ignore `.gents/` get untracked files, so add an ignore rule:

```sh
echo '.gents/' >> .gitignore
```

## Verification builds

Scanners and verifiers read the change only by SHA, never from the working
tree. The verifier has no build sandbox: plugin-job agents carry no workspace
binding, so the host refuses `artifact_write` bash. It reads and reasons
about the pinned code without building or running tests, and the result
lists `verify-<n>: builds not run` in `limitations`.

## Use from a Task

A Task runs `review_changes` and waits for it without churn. This is the
canonical protocol; other packs copy or link it.

1. Call `review_changes` once with `args` (`base`, `head`, `repository`,
   `focus`) and `agent_budget` 32. Take the job address
   `session_id:tool_call_id` from the receipt, pass it whole (readers split
   it at the last `:`) and never address the job by a handle. If the tool
   result is itself a pause or a terminal rather than a receipt, skip to
   step 5.
2. Call `spawn_process` with exactly
   `{"tool_name":"bash_unrestricted","args":{"command":"gents job watch <address> --until-terminal --home <home> --graphql <graphql> --output json"}}`.
3. Call `wait_process` on that process with `timeout_secs` 600. A timeout
   while it runs is the Goal's deferral: end the turn, and when the Goal
   wakes you call `wait_process` again on the same `tool_call_id`; never
   spawn a second watcher.
4. On exit 0, read the job's stored result. Require `status` to be
   `complete` and `head_sha` to equal the exact HEAD you passed. The review
   is clean only when, in addition, `counts.unverified` is 0 and no
   `limitations` entry starts with a lens id (`correctness:`,
   `architecture-reuse:`, `security-concurrency:` or
   `workflow-invariants:`). Unverified candidates are candidates no
   verifier confirmed or refuted, and a lens entry is a scan that failed or
   was blocked, so a review with either is not clean:
   record it as `blocked` with those `limitations` entries, never as a
   pass. The other entries (`verify-<n>: builds not run`,
   `triage did not succeed`) are disclosures and do not by themselves make
   the review not clean. Process exit alone is not evidence, and the
   completion notification is not the result.
5. Any other outcome (a `workflow_*` or `plugin_job_*` code, a paused,
   `awaiting_user`, `failed`, `cancelled` or `timed_out` state, or a non-zero
   watcher exit) is recorded as `blocked` with the code or state. Never
   call the tool again to resume: a Task cannot answer a question.

## Upgrading from 1.x

2.0.0 changes the pack kind from graph to documents, so upgrade with
`gents pack remove gents/code_review` and then `gents pack install`, not
`pack update`. The old `CodeReview*` collections stay dormant: `pack remove`
reports them `retained` and nothing reads them. There is no wipe and no
conversion, and the `review_evidence` plugin is gone.

## Validation

```sh
gents pack build ./packs/gents/code_review
gents pack check ./packs/gents/code_review
gents pack test ./packs/gents/code_review
make test-code_review        # from the repository root
cargo test --manifest-path packs/gents/code_review/plugins/review_changes/Cargo.toml
```

`tests/install.json` installs the pack from its directory into a fresh home,
reinstalls it and removes it; `tests/content.json` asserts the personas' tool
authority, the agent targets, the plugin declaration, that the fixtures are
declared assets and this README's protocol section. `plugins/review_changes/tests/*.json`
are `{input, expect}` job steps run against the built artifact: one per
phase transition, including the budget pause, the correction path and the
windowed panel.

## Operational history

See `../grok_tui_port/run_history.md` for the production Grok review case
study.
