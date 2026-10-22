# Agent options

`agent(prompt, #{ ... })` and each item of `parallel` take these options.

| Option | Effect |
|---|---|
| `label` | Title of the child, cut at 256 B. |
| `phase` | Groups the child under a progress phase. |
| `capability_mode` | `read-only` (default), `read-write`, `execute` or `all`; picks target `wf-<mode>`. |
| `agent_type` | A target id used as given; it replaces `capability_mode`, and the run logs that. |
| `model`, `effort` | Pick a variant of the target: `wf-<mode>@<slot>`, where the slot is `<model>`, `<model>-<effort>` or `<effort>`. It exists only if listed below. |
| `output_schema` | A self-contained JSON Schema up to 64 KiB; `output` is then the validated object. |
| `resume_from` | Sends a new message to a child this run started. |
| `cwd` | A subpath of the caller's cwd; no `..`, no absolute path. |
| `max_output_tokens` | Ignored. |

Refused
- `isolation_worktree`: use `capability_mode` `read-write` in the caller's tree, or split the work.
- `fork_context`: for built-in workflows only.
- A target outside the list below throws `workflow_capability_unavailable` and names the allowed ids.

Personas: `wf-read-only`, `wf-read-write`, `wf-execute` and `wf-all`; `wf-research` (no file or shell access) and `wf-read-only-web` (read-only plus web research, operator opt-in) come from the separate gents/rhai_workflows_web pack when it is installed. When the last line lists targets, the run may use only those ids; otherwise it uses its job ceiling.
