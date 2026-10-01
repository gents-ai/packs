# secscan

The security_scan pack's free mechanical pre-scan. It walks a directory (or
scans file contents handed to it directly) with the same regex matcher
registry deepsec-style: precise-tier matchers (hardcoded secrets, GraphQL
injection, ...) sort before noisy ones (missing auth, path traversal, ...).
Use it to produce the `candidates` payload a scan-plan stage batches, or to
re-run the pre-scan over a specific set of files without touching disk.

## Input

One JSON object with exactly one of:

- `root` (string): an absolute path to a directory to walk. Hidden files are
  skipped, `.gitignore` is honored, files over 1 MiB or not valid UTF-8 are
  skipped. Only a caller that binds a directory for this call can fill
  `root` (see Authority below); this plugin never picks the directory
  itself.
- `files` (array of `{"path": string, "content": string}`): explicit file
  contents to scan; no filesystem access is used.

Both forms accept an optional `max_payload_chars` (integer, default 49152)
capping how much matched-line evidence the payload carries before the
remaining files are demoted to path-only inventory lines.

## Output

One JSON object:

```json
{
  "payload": "files: 2  candidates: 3\nslugs: ...\n...",
  "candidate_total": 3,
  "candidate_files": 2,
  "slug_counts": [{"slug": "secrets-exposure", "count": 1}, ...],
  "slug_counts_line": "secrets-exposure=1",
  "overflow_count": 0
}
```

`payload` is the same model-facing candidates block this crate's own
`format_payload` produced. `slug_counts_line` is `slug_counts` pre-rendered
as `slug=count` pairs joined by a single space, in the same order, for a
caller (a scenario `prepare` step, say) that wants a plain string seed field
rather than the structured array. On invalid input (neither/both of
`root`/`files`, a non-absolute or unreadable `root`, a malformed `files`
entry) the plugin exits non-zero with a one-line explanation on stderr.

## Authority

This plugin declares no standing manifold grant; it declares `bind_dir`
(input field `root`) instead. A caller binds one directory fresh for a
single call - `gents plugin run secscan --bind-dir DIR`, a `gents pack
test` case's own `"bind"` field, or a scenario's `prepare` step - and that
call runs with `root` overwritten to the bound directory's canonical path
and the sandbox's filesystem grant narrowed to exactly it, read-only.
Nothing from a bound call is ever recorded as a standing install grant.
`files` mode needs no binding at all. Declared `limits` (512 MiB, a 300 s
wall clock, and 4 MiB of output - the host's own fixed stdout-capture
ceiling for every compiled plugin) raise a call's budget above the sandbox
default for a whole-repository walk.
