Review the converged Grok TUI port for run {{ event.correlation }}.

<untrusted_convergence>
status={{ doc.status }} head={{ doc.head_sha }}
{{ doc.summary }}
</untrusted_convergence>

Call `read_grok_port_job`, `read_port_convergence_report`, and
`read_port_surface`. Count every executable surface (`implement` or
`shaped-stub`) and put that exact number in `implement_surface_count` and, for
a green report, `live_result_count` (use 1 only if the executable count is
zero). A non-green report uses `live_result_count=1` to close the blocked live
sentinel path.

If convergence is skipped or blocked before any executable unit integrates,
`implement_surface_count` is `0`, not the sentinel document count, and
`live_result_count` is `1` solely to close the downstream sentinel path.

If convergence is not green, its head does not equal current exact HEAD, or
the tracked worktree is dirty, do not review or modify source. Write one final
report with `status=blocked`, zero rounds, the actual head and counts, and the
precise mismatch.

Treat the convergence report as non-green evidence if `tests_run` records any
nonzero exit, failed test, permission error, timeout, or required gate that was
not executed, regardless of its declared `status`. Before a green final report,
independently rerun all three convergence gates, including
`RUSTC_WRAPPER= TMPDIR="$PWD/target" cargo test -p gents-cli --lib grok_shim`.
As in convergence, command exit status is authoritative and failures cannot be
waived as environmental.

The `review-changes` workflow (code_review >=2.0, this pack's dependency) is
installed. Each full review round requires the exact HEAD to equal the first
round's converged head (or the focused fix head on round two). Resolve it, then
run the round:

1. Call `review_changes` with the `args`
   (`base` = `<job.base_sha>`, `head` = `<head-sha>`, `repository` = `.`,
   `focus`) and `agent_budget` 32. Take the job key `session_id:tool_call_id`
   from the receipt, pass it whole (readers split it at the last `:`) and never
   address the job by a handle. If the tool result is itself a pause or a
   terminal rather than a receipt, skip to step 5.
2. Call `spawn_process` exactly as
   `{"tool_name":"bash_unrestricted","args":{"command":"gents job watch KEY --until-terminal --home <job.orchestrator_home> --graphql <job.orchestrator_graphql> --output json"}}`.
   Replace KEY with the whole job key the `review_changes` receipt printed (`session_id:tool_call_id`). For example, with the key `trigger-session:abc123:call456` the command is `gents job watch trigger-session:abc123:call456 --until-terminal --home <job.orchestrator_home> --graphql <job.orchestrator_graphql> --output json`. The command must not contain KEY or `<key>`.
3. Call `wait_process` on that process with `timeout_secs` 600. A timeout
   while it runs is the Goal's deferral: end the turn, and when the Goal wakes
   you call `wait_process` again on the same `tool_call_id`; never spawn a
   second watcher.
4. On exit 0, run `gents job show KEY --home <job.orchestrator_home> --graphql <job.orchestrator_graphql> --output json` the same way, with KEY replaced the same way, and read its `result`. If its state is still `running`, that is not an outcome: go back to step 3. Require `status` to be
   `complete` and `head_sha` to equal the exact HEAD you passed. The review
   is clean only when, in addition, `counts.unverified` is 0 and no
   `limitations` entry starts with a lens id (`correctness:`,
   `architecture-reuse:`, `security-concurrency:` or
   `workflow-invariants:`). Unverified candidates are candidates no
   verifier confirmed or refuted, and a lens entry is a scan that failed,
   was blocked or read other trees, so a review with either is not clean:
   record it as `blocked` with those `limitations` entries, never as a
   pass. The other entries (`verify-<n>: builds not run`,
   `triage did not succeed`) are disclosures and do not by themselves make
   the review not clean. Process exit alone is not evidence, and the
   completion notification is not the result.
5. Any other outcome (a `plugin_job_*` code, a paused, `awaiting_user`, `failed`, `cancelled` or `timed_out` state, or a non-zero
   watcher exit) is recorded as `blocked` with the code or state. Never
   resume: a Task cannot answer a question.
6. If round one has confirmed findings, inspect each against current source,
   make focused fixes, run affected tests and `cargo fmt --all --check`, stage
   only explicit fix paths, create focused commits, and start one fresh full
   review against the new exact HEAD.

At most two full review rounds are allowed. Before green, run the repository
foundation gates `cargo test -p gents` and
`cargo check --workspace --all-targets`, require zero confirmed findings and a
clean tracked worktree, and record exact HEAD. Never push, open a PR, or merge.
Use `spawn_process` with `tool_name: "bash_unrestricted"` for any cargo command
that may exceed the foreground bash timeout, then use
`wait_process`/`read_process` until that exact managed process is terminal. Do
not use `nohup`, shell `&`, or untracked gate logs. Require the managed process
terminal result to report exit code zero.

Call `write_port_final_review_report` exactly once. Use `status=green` only
when the durable review and both gates pass; otherwise block with honest round,
finding, and test evidence. Do not supply run_id.

After the terminal final-review report is durably written, call `update_goal`
with `status="complete"`. Green and blocked reports both complete this stage;
never complete the goal before the review and foundation gates reach that recorded outcome.
