# review_changes

Review a git change through four lenses, adversarially verify every candidate and triage the
result. The review runs in the background as a job: this call returns a receipt with the job
address and the result arrives later as a message. End your turn after the call.

## Input

`args` (all optional): `base` (default `origin/main`), `head` (default `HEAD`), `repository`
(a relative subpath of your cwd, default `.`), `focus` (what the scanners should weigh).
`agent_budget` (default 128) is the number of child agents the run may start; it needs at
least 22, or the run pauses with `budget_exceeded` before any agent starts.

## Output

The result is `{status, base_sha, head_sha, diff_id, counts, findings, report, limitations, path}`;
`path` is the Markdown report under `<cwd>/.gents/workflows/runs/<tool_call_id>/review.md`.
