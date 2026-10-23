# workflow

Run a multi-step, multi-agent workflow you write yourself, as a background job.
Pass the workflow as `script`: a JavaScript module whose `module.exports` is a
step function `(input) => output`. The host calls it once per step with its
view of the run and acts on what it returns. The call returns a receipt at
once; the result arrives later as a message. End your turn after the call.

## Input

- `script`: the JavaScript source (required).
- `args`: an object your step function reads as `input.args`; `args.handle`
  names the run.
- `agent_budget`: how many child agents the run may start (default 128).

## The step function

`input` has `pin` (null on the first step), `args`, `agent_budget`, `children`
(slot -> `{state: live|completed|failed|interrupted, title, usage}`),
`ceiling` (`targets`, `max_live`, `remaining_spawns`, `result_bytes`), `state`
(what your last step returned as `effects.state`, or null) and, on a round,
`job_results`.

`output` is exactly one of:
- `{effects: {spawn: [{slot, attempt: 0, target, prompt, title}], message: [{slot, text}], state, progress}}`
  to start or steer children (`target` is one of `ceiling.targets`);
- `{job_calls: {requests: [{id: "text", child_output: {slots: [...], offset: 0, limit: input.ceiling.result_bytes}}]}}`
  to read completed children's text, answered in `input.job_results.text.result.slots[slot].text`;
- `{result: <any JSON>}` to finish; `{suspend: {kind: "pause", message}}` to pause;
  `{fail: {code, message}}` to fail.

A step with live children and nothing new to do returns `{effects: {}}`.

## Example: fan out, read, judge

Three workers each take one side; a fourth judges from their texts. Every
phase is read off `input.children`, so the function needs no `state`.

```js
module.exports = (input) => {
  const T = input.ceiling.targets[0];
  const sides = (input.args && input.args.sides) || ["economic", "military", "cultural"];
  const slots = sides.map((_, i) => "side" + i);
  const c = input.children;
  const done = (slot) => c[slot] && c[slot].state !== "live";
  const read = (want) => ({ job_calls: { requests: [{ id: "text",
    child_output: { slots: want, offset: 0, limit: input.ceiling.result_bytes } }] } });
  const texts = () => input.job_results && input.job_results.text.result.slots;

  // Step 0: nothing has run yet.
  if (!input.pin) {
    return { effects: { spawn: sides.map((side, i) => ({
      slot: slots[i], attempt: 0, target: T, title: "Argue " + side,
      prompt: "In 200 words, argue that the " + side + " cause was the main cause of the fall of Rome. Answer with the argument only.",
    })) } };
  }
  // Phase 1: the sides are running.
  if (!slots.every(done)) return { effects: {} };
  // Phase 2: the sides are done and the judge has not started: read their texts, then spawn the judge.
  if (!c.judge) {
    const t = texts();
    if (!t) return read(slots);
    const brief = slots.map((s, i) => "## " + sides[i] + "\n" + (t[s] ? t[s].text : "(no answer)")).join("\n\n");
    return { effects: { spawn: [{ slot: "judge", attempt: 0, target: T, title: "Judge",
      prompt: "Three arguments follow. Say which is strongest and why, in 150 words.\n\n" + brief }] } };
  }
  // Phase 3: the judge is running, or done.
  if (!done("judge")) return { effects: {} };
  const t = texts();
  if (!t || !t.judge) return read(["judge"]);
  return { result: { verdict: t.judge.text, sides } };
};
```

## What the host refuses

- Any key beside `effects` other than the ones listed; `state` goes inside
  `effects`, not beside it.
- A spawn without `attempt`, `target`, `prompt` or `title`, or with a `target`
  not in `ceiling.targets`.
- A step that spawns nothing new and sends no message while no child is live:
  the run fails as stalled. Spawning a slot and attempt that already ran is not
  new, so decide the phase from `input.children`, never by re-sending step 0.
- Reading a child's text from `children[slot]`: it is not there. Ask with
  `job_calls` and read `job_results` on the next call, as above.
- A worker whose prompt needs files or tools it lacks answers that it cannot;
  prompts that ask for reasoning or prose are fine.

## Promoting a workflow to a pack

When asked to promote a workflow you ran, make it an installed plugin that no
longer needs this tool or the dynamic policy. You need a bash tool that may
write under the packs checkout; if yours is read-only, say so and stop. Steps:

1. In `packs/gents` of the checkout run
   `gents pack new <name> --template workflow --language js`. It writes
   `plugins/<name>/source/main.js` (a `module.exports` step function),
   `TOOL.md`, a step-0 fixture under `plugins/<name>/tests/`, a worker persona
   and target named `<name-with-dashes>-worker`, and the caller Tools template.
2. Put your step function into `source/main.js` in place of the skeleton's
   `step0` and `advance`. Keep `module.exports = (input) => ...`. Keep the
   step-0 `pin` object (`script_id`, `source_digest`, `agent_budget`,
   `handle`): the host stamps it for a dynamic run, an installed plugin writes
   it. Spawn on the pack's `TARGET`, not on `wf-read-only`. Nothing in the
   file may read stdin or write stdout; `gents pack build` wraps it.
3. Rewrite the fixture's `expect` to what your step 0 returns, then run
   `gents pack build packs/gents/<name>`, `gents pack test packs/gents/<name>`
   and `gents pack check packs/gents/<name>` until all pass.
4. Stop and ask the operator to finish; you cannot grant the pack job
   authority yourself. Give them these two commands, filled in:
   `gents pack install packs/gents/<name> --home <home> --node-did <did> --grant-authority --inference-slot worker=<profile>`
   and `gents pack wire gents/<name> --agent <agent> --targets-from gents/<name> --home <home> --node-did <did>`.
   After that the workflow is the tool `<name>`, called with `args` and
   `agent_budget` only.
