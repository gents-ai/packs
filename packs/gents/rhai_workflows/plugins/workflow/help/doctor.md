# Doctor

Start here when `workflow_policy_denied`, `workflow_not_available` or `plugin_job_not_available` appears.

The usual fix: the operator wires the engine for the agent and trusts the pack that holds the workflows:
`gents pack wire gents/workflow --agent <A> --targets-from <pack>`.
With no trusted pack and `dynamic` set to `check`, the engine runs nothing.

An engine installed with no provenance record reads as installed through self-config, and the host refuses it. The operator reinstalls it with `gents pack install <source> --grant-authority`.

The facts below are what this engine can see. For the host's side, the operator runs `gents diagnose plugin_jobs`.
