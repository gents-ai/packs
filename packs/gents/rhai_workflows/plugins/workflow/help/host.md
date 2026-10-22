# Host functions

Calls that take a position in the journal: agent, parallel, budget, await_user, write_scratch_file, read_scratch_file. The rest are pure.

| Function | What it does |
|---|---|
| `agent(prompt[, opts])` | Starts one child agent. Returns `#{agent_id, success, output, cancelled, tokens_used, duration_ms}`. Options: `help agent`. |
| `parallel([opts, ...])` | Runs maps of agent options as a panel. A barrier: results come back in input order, `()` for any item that did not succeed. Wide panels start as live slots free up. |
| `phase(title)`, `log(msg)` | Progress only. Each is cut at 256 B with the cut named. |
| `complete(v)` | Ends the run with `v`. Integers above i64::MAX become floats, so carry ids as strings. |
| `pause(kind, msg)` | Holds the run; it fires again on every resume. |
| `await_user(kind, msg)` | Asks a question; returns the answer text once given. |
| `budget()` | `#{total, spent, remaining, reserved}`; `reserved` is always 0. |
| `write_scratch_file(name, content)` | Writes a file under the run's scratch directory and returns its absolute path. |
| `read_scratch_file(name)` | Reads a file this run wrote. |
| `fingerprint(text)` | First 16 hex digits of the SHA-256 of the text. |
| `json_encode(v)` | Compact JSON with sorted keys. |
| `render_template(name, map)` | Fills a bundled template; `{{key}}` is replaced. Templates: `report`, `research_brief`, `review_findings`. An unknown name lists the known ones. |
| `git_diff_since(rev)` | Throws; ask a read-only agent to run `git diff <rev>` instead. |

Where this host differs
- `budget()` is computed on each replay, so after `workflow resume <key> --budget <n>` it shows the raised cap.
- Scratch paths are absolute and named by the tool-call id of the run: `<cwd>/.gents/workflows/runs/<tool_call_id>/<name>`. Names are one path component of a-z, 0-9, `.`, `_`, `-`.
- Past the budget, `agent()` or a panel that would cross it starts nothing and pauses the run with kind `budget_exceeded`; resume it with a higher `--budget`. A script's own `pause("budget_exceeded", msg)` ends the same way.
