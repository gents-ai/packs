# Create a workflow

Use this when the user wants a repeatable multi-agent job written as a Rhai script and run with the `workflow` tool.
Read `workflow help language`, `workflow help host` and `workflow help agent` once before writing; this page is the short path.

## Steps

1. Say in two lines what the script decides, and which agent steps it needs.
2. Write the script. It begins with `let meta = #{ name: "...", description: "..." };` as a pure literal, then the body. Inputs arrive in `args`.
3. Run `workflow check` with the source in `script`. It validates the script and smoke-runs it with canned agent answers, then returns a `script_id`. Fix what it names and check again until it passes.
4. Run it with `workflow run`, passing the same `script` (or `path`, or a kept name) and the inputs in `args`. The call returns at once with a receipt. End your turn: the result arrives later as a message.
5. If the user wants to reuse it, `workflow keep <script_id>` saves it under `meta.name`. A script that should live with a project goes in `.gents/workflows/<name>.rhai`, one file per workflow.

## Writing rules

- Keep the script deterministic. It is replayed from the top on every step, and each host call is answered from a journal. Pass any timestamp or random seed in through `args`.
- End with `complete(value)`. Do not call `exit()`: the name is free to use as a variable, but calling it throws. `timestamp()` and `sleep()` throw too.
- `pause(kind, message)` and `await_user(kind, message)` both take two arguments. `await_user` returns the answer text once the user replies.
- A script may gate its own budget: call `pause("budget_exceeded", message)` before the first `agent()` when the plan is larger than the user has agreed to. The run resumes with `workflow resume <key> --budget <n>`, and `budget()` then shows the raised total.
- `agent(prompt, opts)` returns a map; a failed child is data, so test `success` before using `output`. Give `output_schema` when the next step reads fields.
- Pick the narrowest `capability_mode` that does the job: `read-only` (the default), `read-write`, `execute` or `all`. Use `parallel([...])` for independent steps.
- Three names are refused: the agent options `isolation_worktree` and `fork_context`, and the function `git_diff_since`. For a diff, ask a read-only agent to run `git diff <rev>`. For isolation, split the work or use `read-write` in the caller's tree.
- Scratch files: `write_scratch_file(name, content)` returns an absolute path, and the directory is named by the run's tool-call id, not by its handle. Use the returned path as given.

## Running and following a run

- A run that pauses, asks the user a question, or writes scratch before its first `agent()` is still a background run, started and tracked like any other.
- The receipt prints the job address `session_id:tool_call_id`. Pass that address (or the handle) to `workflow status|result|stop|pause|resume|answer`. Split an address at its last colon, because the session id may contain colons.
- `/workflows` in the client is a catalog of what can run; it is not a control for a running job. Use the verbs above.
- On any error, read the `Next:` call at the end of the message and do it. `workflow help errors` lists the codes.

## Skeleton

```rhai
let meta = #{
    name: "review-files",
    description: "Review each file named in args.files and merge the findings",
};

if budget().remaining < 3 {
    pause("budget_exceeded", "Needs at least three agent calls");
}

let panel = args.files.map(|f| #{
    prompt: "Review " + f + " and list defects",
    label: f,
    capability_mode: "read-only",
});
let found = parallel(panel);
complete(found.filter(|r| r != ()).map(|r| r.output));
```
