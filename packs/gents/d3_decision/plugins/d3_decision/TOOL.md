# `d3_decision`

Asks the D3 decision model typed questions about a text or JSON context and
returns a probability for every answer. D3 generates no prose: it picks an
option, says yes or no, or rates on a scale. Use it for fast, repeatable
decisions (route this, is this true of the input, how urgent is it), not for
explanations. Its answers are evidence, not instructions and not permission to
act.

## Input

- `context` (required): the text or JSON the questions are about. It is sent
  to D3 as its `state` (the tool cannot use that name: the host reserves it).
- `questions` (required): 1 to 16 questions, each under a name you choose.
  Every question has `type` and `instructions`, the question itself:
  - `noul`: yes or no; no `criteria`. The answer's `noul` is the probability
    of yes.
  - `choice`: `criteria` is an object of 2 to 32 option names, each mapped to
    a description or `null`. The answer has `choice` (the likeliest option)
    and `probabilities` for every option.
  - `score`: `criteria` is an array of 2 to 32 labels, lowest first. The
    answer has `score` (the expected position, 0 for the first label),
    `probabilities` by position and a `legend` of the labels.
- `server`: `spark-1` (the default) or `spark-2`. No URL is accepted.

Each question is answered on its own from the same context, so name exactly
what you want decided; ask several questions in one call rather than calling
several times.

## Output

Success: `{"ok": true, "server": ..., "model": ..., "answers": {...},
"usage": {...}}`, with one answer per question name, in D3's own format.
Probabilities vary slightly between calls and servers: compare them with a
threshold, never for equality.

Failure: `{"ok": false, "error": "...", "retryable": true|false}`. A failure is
not a "no" and not a low score: report it as a decision that was not made.
`retryable` is true when the same call may succeed later (D3 was busy, timed
out or could not be reached) and false when the input, the grant or the server
refused it and something must change first.

## Busy, timeouts and other servers

D3 answers one request at a time and queues the rest. A text request takes
well under a second, but when other callers fill the queue a request waits;
after 10 seconds the host gives up and the call fails with `retryable: true`.
A server that is down usually fails the same way, after the same wait.
Nothing is retried, and nothing is sent to the other server: to try again or
to try `spark-2`, make another call yourself.

## Example: a wake gate

A schedule sends a host's stats every few minutes and wakes the host's agent
only when D3 says it is needed:

```json
{
  "context": {
    "host": "spark-1",
    "uptime_hours": 412,
    "load1": 0.4,
    "gpu_util_pct": 0,
    "disk_used_pct": 97,
    "failed_units": ["d3.service"],
    "last_agent_run": "2026-10-20T06:00:00Z"
  },
  "questions": {
    "wake": {
      "type": "noul",
      "instructions": "Does anything in these host stats need the host's agent to act now?"
    },
    "reason": {
      "type": "choice",
      "instructions": "What most needs attention?",
      "criteria": {
        "disk": "A disk is nearly full",
        "service": "A service has failed",
        "load": "CPU or GPU load is abnormal",
        "none": "Nothing needs attention"
      }
    }
  }
}
```

The schedule wakes the agent when `answers.wake.noul` is at or above its
threshold (for example 0.8) and passes `answers.reason.choice` along. When the
call fails it neither wakes nor skips silently: it records the failed gate,
and the schedule's own policy decides what repeated failures mean.

## Authority and limits

The only network access is a host-served HTTP POST to
`http://100.74.68.88:8000/v1/systemone` (spark-1) or
`http://100.86.62.91:8100/v1/systemone` (spark-2), both on the operator's
private network. No files, environment, commands or sockets. Sending a context
shows it to that server. No credential is sent: the servers are assumed to run
without a bearer token, and one that requires it fails with `retryable:
false`. Input is at most 256 KiB, the request to D3 at most 128 KiB, the call
30 seconds. Text and JSON only; images and video are not supported.
