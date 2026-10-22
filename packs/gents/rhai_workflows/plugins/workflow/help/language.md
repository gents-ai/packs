# Workflow language

A workflow is one Rhai script. It runs in a sandbox and must be deterministic: the engine replays it from the start on every step and answers each host call from the journal.

Shape
- The script starts with `let meta = #{ ... };`, a pure literal and nothing else on that line: `name` (lower-case words joined by `-`), `description`, optional `when_to_use`, optional `phases` as `[#{title, detail}]` with unique titles.
- Inputs arrive in the map `args`, or `()` when none were given.
- Ordinary Rhai applies: let, if, for, while, functions, maps, arrays, closures, try/catch.
- Do not name a function after a host function or stub (`agent`, `phase`, `complete`, `timestamp`, ...): `check` refuses it.

Ending a run
- `complete(value)` ends it with a result.
- `pause(kind, message)` holds it; kinds are user, back_off, no_progress, verification, infra and budget_exceeded.
- `await_user(kind, message)` asks the operator a question and returns the answer.
- These three, and the signal that a child is still pending, cannot be caught: a try/catch around `agent()` never swallows them.
- Anything else a host call refuses throws an error you can catch. An agent that fails is data: check `success` on its result.

Stubs
- `timestamp()` throws: wall-clock time breaks replay, so pass a timestamp in `args`.
- `sleep()` throws: host calls already wait for their work.
- `exit()` throws: end with `complete` or `pause`.
- `eval`, `import` and modules are off. `print` and `debug` output is discarded: use `log(msg)` to write to the run's progress.

Limits
- 50,000,000 operations per step, strings up to 4 MiB, arrays and maps up to 100,000 entries.
- A run spends from an agent budget (default 128, set with `agent_budget` 1 to 1024).

Try it: `workflow check --script` with the source in `script` validates and smoke-runs it, then `workflow run` with the same `script`, or `workflow keep <script_id>` and then `workflow run <name>`.
