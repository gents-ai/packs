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
  skipped. Requires the caller to have granted this plugin read access to
  the path (see Authority below: none is granted by default today).
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
  "overflow_count": 0
}
```

`payload` is the same model-facing candidates block the CLI's `format_payload`
produced. On invalid input (neither/both of `root`/`files`, a non-absolute or
unreadable `root`, a malformed `files` entry) the plugin exits non-zero with
a one-line explanation on stderr.

## Authority

No host access is declared in this pack's manifest today: `gents` cannot yet
bind a per-run directory to a plugin's manifold, so `root` mode only works
once a caller installs the pack with an explicit `fs.ReadOnly` grant of its
own. `files` mode needs no grant at all.
