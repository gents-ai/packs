# Errors

Every failure reads `<code>: <what>. Next: <one call>`. Do the call it names.

Engine codes
- `workflow_invalid_input`: a verb, operand or argument is wrong. Next: `workflow help <verb>`.
- `workflow_not_available`: this call has no job settings, or the verb needs them. Next: `workflow help doctor`.
- `workflow_launch_failed`: the script stopped without complete(), threw an uncaught error, or the host could not serve the step. Next: the call the message names (fix the script, then `workflow check`).
- `workflow_resolve_failed`: no script matched the name, path or form. Next: `workflow list`.
- `workflow_resume_unknown_run`: no run note for that key. Next: `workflow status <key>`.
- `workflow_control_unknown_run`: the key is not a run of this session. Next: `workflow status`.
- `workflow_control_not_applicable`: the run is in a state this verb cannot act on. Next: `workflow status <key>`.
- `workflow_validation_failed`: the script breaks a rule. Next: `workflow check`, then fix what it names.
- `workflows_disabled`: workflows are switched off here. Next: `workflow help doctor`.
- `workflow_depth_exceeded`: a job's agent tried to start a workflow. Next: ask the user to run it from their own session.
- `workflow_policy_denied`: the operator's policy forbids this start. Next: `workflow help lifecycle`.
- `workflow_capability_unavailable`: the target is outside the ceiling. Next: `workflow help agent`.

Host terminal codes (they end the run)
- `plugin_job_stalled`: Next: `workflow status <key>`, then report the script.
- `plugin_job_policy_denied`: Next: `workflow help agent`, and ask the operator to widen `job.*`. It also ends a run whose engine refused while children were live, so read `workflow status <key>` first.
- `plugin_job_not_available`: Next: `workflow help doctor`; the operator restores the job block or reinstalls the engine with `gents pack install <source> --grant-authority`.
- `plugin_job_engine_changed`: Next: `workflow resume <key> --engine current`.
- `plugin_job_timed_out`: Next: `workflow resume <key>`.
- `plugin_job_depth_exceeded`: Next: run the workflow from the top-level session.
- `plugin_job_step_failed`: three steps in a row failed. Next: `workflow status <key>`, then `workflow resume <key>`.
