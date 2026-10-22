# Compatibility with Grok workflows

Scripts written for Grok's workflow tool mostly run unchanged.

Same behaviour
- The `meta` literal, `args`, the shape of an `agent()` result, `parallel()` order and `()` slots, `phase`, `log`, `complete`, `pause`, `await_user`, `read_scratch_file`, `fingerprint`, `json_encode`.
- `timestamp()`, `sleep()` and `exit()` throw, with this engine's own wording.
- `capability_mode`, `agent_type`, `model` and `effort` pick targets, as long as the operator configured them.
- `isolation_worktree` and `fork_context` are refused; `max_output_tokens` is ignored.
- `check` plays the part of `validate_only`.

Declared divergences
- `budget()` is computed from the journal so far and the current pin, so a resume with a higher `--budget` also raises `total` for earlier calls. A script's own budget gate lifts on resume.
- `write_scratch_file` returns an absolute path, and it works before the first `agent()`.
- Nesting: Grok refuses a workflow started by any subagent. Here only a caller descended from a workflow job is refused (`workflow_depth_exceeded`); a subagent of the top-level session may start one.
- `pause` also accepts the kind `budget_exceeded`.
- A `parallel()` panel holds a cap of live children and starts the rest as slots free up.
- Discovery order is pack, then project, then kept. There is no dashboard; `list` is the catalog.
- Journals are not interchangeable between the two tools.

A script that relies on features only Grok has is Grok-only; `check` or `run` tells you which call fails.
