# D3 decision pack

Asks the D3 decision model (`vllm-sr/d3`) typed questions about a text or
JSON context and returns a probability for every answer: pick one of several
options (`choice`), yes or no (`noul`), or a rating on a scale (`score`). D3
generates no prose and answers a text request in well under a second, which
makes it a decision primitive for graphs, schedules and agents rather than a
chat model. The motivating use is a
fleet wake gate: a frequent schedule sends a host's stats and asks whether the
host's agent needs to wake (see the example in `plugins/d3_decision/TOOL.md`).

The pack ships one plugin, `d3_decision`, and no agent or documents. D3 itself
is not part of the pack: it runs on operator-owned servers the plugin reaches
through the host.

## Use it from the CLI

```sh
gents pack install ./packs/gents/d3_decision --home <home> --preview
gents pack install ./packs/gents/d3_decision --home <home> --grant-authority
gents plugin run gents/d3_decision --home <home> --input '{
  "context": "The order arrived damaged and the customer has a receipt.",
  "questions": {
    "receipt": {"type": "noul", "instructions": "Does the customer have a receipt?"},
    "route": {"type": "choice", "instructions": "Which team handles this?",
              "criteria": {"returns": "Damaged deliveries", "billing": "Payment charges"}}
  }
}'
```

## Use it as a model tool

Grant `{"plugin": "gents/d3_decision"}` in an agent's Tools document. The model
sees the input schema and `plugins/d3_decision/TOOL.md`, which states the
contract: three question kinds, the success and failure shapes, and that a
failure is never a "no".

## Installation and prerequisites

`--preview` shows the network grant before anything is written; installing
needs the operator to approve it (`--grant-authority`, or the approval prompt).
There is no inference slot, key or model download. The D3 servers must be up
and reachable from the node over the operator's Tailscale network:

| `server` | Origin | Route |
| --- | --- | --- |
| `spark-1` (default) | `http://100.74.68.88:8000` | `POST /v1/systemone` |
| `spark-2` | `http://100.86.62.91:8000` | `POST /v1/systemone` |

`GET /health` on either reports `"status": "ready"` and whether it requires
authentication.

## Authority

The plugin opens no socket. It hands the host one HTTP request per call and the
host sends it only if the URL is inside the manifest's `OutboundHttp` grant,
which names exactly the two origins above as IP literals (a hostname grant
reaches only public addresses, so private Tailscale addresses must be granted
as literals). The caller picks a symbolic server, never a URL. No files,
environment, commands, crypto or child processes are requested. If an address
changes, change `SERVERS` in `plugins/d3_decision/source/main.rs` and the grant
together (a unit test fails when they disagree), rebuild, and re-approve.

No credential is sent. The servers run without a bearer token today (`/health`
reports `"authentication": false`). D3 can be configured to require one, but a
plugin has no supported place to keep a secret: `env` grants would put it in
the process environment and a bound file would put it on a path the caller
names. A server that requires a token fails the call with `retryable: false`
and says so; supporting one needs a host-held secret mechanism first.

Sending a context shows it to the chosen server.

## Inputs and outputs

See `plugins/d3_decision/TOOL.md` for every field. In short: `context` (text or
JSON, sent to D3 as its `state`), `questions` (1 to 16 named questions of type
`choice`, `noul` or `score`), and `server`. Success is `{"ok": true, "server",
"model", "answers", "usage"}` with D3's answers as it returned them, checked to
hold one answer of the asked kind per question. Failure is `{"ok": false,
"error", "retryable"}`.

## Completion and failure

One call is one request to one server. D3 serves one request at a time and
queues the rest, so a busy server shows up as a slow answer; the host gives up
after 10 seconds (`http_timeout_ms`) and the call fails with `retryable: true`
and a message that says D3 was busy, stalled or down (a server off the
network times out the same way). A refused connection, a connection D3
dropped before answering (it restarted or was killed mid-request), HTTP 429 or
503, and HTTP 5xx are also `retryable: true`; a rejected question (HTTP 400 or
422, with D3's reason), a request too large to answer in time, a required
token, a refused grant or a malformed response are `retryable: false`.

D3 reads the whole context once for every question and keeps working on a
request after the host has stopped waiting for it, so a request it cannot
answer inside the timeout costs every other caller that time too. On spark-1
it takes about 1 ms per input token per question (0.7 s for a 250-byte host
snapshot with three questions, 6.8 s for 4 KiB with three, 34 s for 16 KiB
with three). The plugin therefore refuses, before sending anything, a call
whose context bytes plus question bytes, summed over its questions, pass
12 KiB. Nothing is retried and nothing is sent to the
other server: retrying, or asking `spark-2`, is the caller's explicit choice.
There is no `auto` server for the same reason: no pack here fails over between
endpoints implicitly, and a wake gate should see that its server is down.

## Validation

```sh
cargo test --manifest-path packs/gents/d3_decision/plugins/d3_decision/Cargo.toml
gents pack check packs/gents/d3_decision
gents pack test packs/gents/d3_decision
GENTS=gents scripts/test-pack.sh packs/gents/d3_decision
```

The suite never contacts D3: the WASM cases in `plugins/d3_decision/tests/`
fail validation before any request, and the native tests feed the plugin the
host's answers (success, timeout, unreachable, HTTP refusals, missing or
mistyped answers). Check a live server by hand with semantic assertions, not
exact floats, since probabilities differ slightly between calls and servers:

```sh
gents plugin run gents/d3_decision --home <home> --input '{"context": "The customer has a receipt.",
  "questions": {"receipt": {"type": "noul", "instructions": "Does the customer have a receipt?"}}}' \
  | jq -e '.ok and .answers.receipt.type == "noul" and .answers.receipt.noul > 0.9'
```

## Operational history

0.1.1: a dropped connection is retryable, and the request size bound counts
context once per question (12 KiB of work, replacing the 128 KiB body cap
that D3 could not answer inside the timeout). Measured on spark-1 under
gents 0.21.0-rc2: answers are deterministic for the same request, and a
request abandoned by its client still runs to completion on D3.

0.1.0: first release, ported from the gents 0.21.0-rc0 pack-authoring audit,
where it answered choice, noul and score questions live on both servers.
Live check at release (2026-10-20, gents 0.21.0-rc0): on spark-1 a wake-gate
call with a 97% full disk and a failed unit answered `wake` above 0.99 and
`reason` disk or service, and healthy stats answered `wake` below 0.01;
spark-2 was down and the call failed with `retryable: true` after the 10 s
timeout, sending nothing to spark-1.
