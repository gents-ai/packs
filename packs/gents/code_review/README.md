# Code review

Four-stage code review personas (recon, scan, verify, triage) and the
`review-changes` workflow that drives them, installed as agent targets.

The pack is a `documents` pack. It ships the workflow script
(`workflows/review_changes.rhai`, listed in `workflows/index.json`), four
personas and one agent target per persona (`code-review-recon`,
`code-review-scan`, `code-review-verify`, `code-review-triage`). It has no
engine dependency and no `job` block: the operator wires it to an agent.

## Operator steps

1. Install the workflow engine and wire it to an agent. On a new home,
   `gents init --workflows` does both. On an existing home, run
   `gents pack install gents/rhai_workflows --wire <agent> --grant-authority`.
2. Install this pack as the operator, so its origin is `installed`.
   Steps 1 and 2 may run in either order.

   ```sh
   gents pack install gents/code_review --home ./.gents --node-did "$REVIEW_NODE_DID" \
     --inference-slot coordinator=claude-coordinator \
     --inference-slot worker=glm-worker \
     --inference-slot verifier=grok-verifier
   ```

3. Grant the agent the pack's four targets and trust the pack:

   ```sh
   gents pack wire gents/workflow --agent <A> --targets-from gents/code_review
   ```

   This adds the four target ids to the agent's `job.agent_targets` and
   `AgentTools.target_ids`, and adds `gents/code_review` to
   `job.policy.trusted_packs`. No `gents config tools` round trip is needed.
4. Bind the three inference slots (shown in step 2). `coordinator` serves
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

Reviewed source stays read-only throughout, and review output is evidence,
not permission to merge.

## Args and defaults

| Arg | Default | Meaning |
|---|---|---|
| `base` | `origin/main` | Base ref of the change |
| `head` | `HEAD` | Head ref of the change |
| `repository` | `.` | Repository to review: a relative subpath of the caller's cwd, without `..` |
| `focus` | a general correctness, safety, durability and maintainability brief | Operator focus, passed to the scanners as JSON text |

The workflow needs `agent_budget` of at least 22 (recon, four scans, up to
16 verifiers and triage). With less it pauses with `budget_exceeded` before
any agent runs, and `workflow resume <key> --budget 22` continues it.

## Result contract

On completion the workflow returns:

- `status`: `complete` or `blocked`
- `base_sha`, `head_sha`, `diff_id`: the pinned change and its fingerprint
- `counts`: `candidate`, `confirmed`, `refuted`, `unverified`, `high_priority`
- `findings`: the confirmed findings
- `report`: the triage report, including `merge_recommendation`
- `limitations`: every blocked lens, unverified candidate or other gap
- `path`: the scratch report, see below

Read the full value with `workflow result <key>`. The background completion
notification is capped at 4,000 B and is not the result.

A finding is confirmed only when its verifier returned `confirmed` with
confidence of at least 80 and non-empty evidence, and the verifier resolved
the same trees as recon. Zero findings is distinct from a blocked review.

## Scratch report

The report is written to `<cwd>/.gents/workflows/runs/<tool_call_id>/review.md`,
where `cwd` is the reviewed repository or an ancestor of it. Repositories that
do not already ignore `.gents/` get untracked files, so add an ignore rule:

```sh
echo '.gents/' >> .gitignore
```

The review itself reads git objects by SHA and never sees these files.

## Verification builds

Scanners and verifiers read the change only by SHA, never from the working
tree. The verifier has no build sandbox: plugin-job agents carry no workspace
binding, so the host refuses `artifact_write` bash. It reads and reasons
about the pinned code without building or running tests, and the result
lists `verify-<n>: builds not run` in `limitations`.

## Use from a Task

A Task runs `review-changes` and waits for it without churn. This is the
canonical protocol; other packs copy or link it.

1. Call `workflow` with `argv ["run","review-changes"]`, the `args`
   (`base`, `head`, `repository`, `focus`) and `agent_budget` 32. Take the
   job key `session_id:tool_call_id` from the receipt, pass it whole (readers
   split it at the last `:`) and never address the job by a handle. If the
   tool result is itself a pause or a terminal rather than a receipt, skip to
   step 5.
2. Call `spawn_process` with exactly
   `{"tool_name":"bash_unrestricted","args":{"command":"gents job watch <key> --until-terminal --home <home> --graphql <graphql> --output json"}}`.
3. Call `wait_process` on that process. A timeout while it runs is the
   Goal's deferral, so ending the turn after a timed-out wait is safe.
4. On exit 0, call `workflow result <key>`. Require `status` to be
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
   resume: a Task cannot answer a question.

## Upgrading from 1.x

2.0.0 changes the pack kind from graph to documents, so upgrade with
`gents pack remove gents/code_review` and then `gents pack install`, not
`pack update`. The old `CodeReview*` collections stay dormant: `pack remove`
reports them `retained` and nothing reads them. There is no wipe and no
conversion, and the `review_evidence` plugin is gone.

## Validation

```sh
gents pack check ./packs/gents/code_review
gents pack test ./packs/gents/code_review
make test-code_review        # from the repository root
```

`tests/install.json` installs the pack from its directory into a fresh home,
reinstalls it and removes it; `tests/content.json` asserts the personas' tool
authority, the agent targets, the phase wiring of the script and this
README's protocol section. The workflow index check runs for every pack that
ships `workflows/index.json`.

## Operational history

See `../grok_tui_port/run_history.md` for the production Grok review case
study.
