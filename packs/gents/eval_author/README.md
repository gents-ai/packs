# Eval author

The eval author is the model side of `gents eval init`. It is a single
agent, `eval-author`, with a fixed system prompt and no tools: it cannot
read or write documents, files or configuration. It interviews an operator
about one subject agent and drafts an eval definition - cases that will
later run against that subject and grade what it does.

## Installation

```bash
gents pack install ./packs/gents/eval_author --home <home> --inference-slot author=<profile_id>
gents pack install gents/eval_author --home <home> --inference-slot author=<profile_id>   # once published to the registry
```

`gents eval init` installs the pack into the operator's home once,
idempotently, and binds that slot to `--profile` or the home's default
profile.

## Bindings and prerequisites

The pack declares one inference slot, `author`, bound to the `eval-author`
agent. Nothing else is configurable: the agent's context is the
authoring contract in `agent_behaviors/eval_author/system_prompt.md` (the
case vocabulary, the worked example, the seven rules drafts are held to, the
five interview questions, and the draft reply format).

## Authority

`eval-author-tools` grants no host bash and no file, datastore or self-config
tools.

## Inputs and outputs

```
gents eval init <subject-pack> --out <dir> [--agent <id>] [--profile <profile_id>] [--pilot]
```

The command opens a fresh `AgentSession` against the served home, sends the
subject dossier and the check catalog as the first turn, and runs an ordinary
chat-turn loop between the operator and the author. A reply that contains
exactly one fenced `json` block is a draft.

`gents eval checks` prints the same catalog the author sees.

## Completion and failure

The CLI - never the author - validates a draft: check names against the
catalog, params against each check's schema, captures against the subject's
collections and fields, split and case-id shape, and a full pack-loader round
trip in a scratch directory. Only a draft that survives every step is written
under `--out`. The author holds no write grants at any point; nothing reaches
disk or live configuration before validation passes.

## Validation

```bash
gents pack check ./packs/gents/eval_author
gents pack test ./packs/gents/eval_author
make test-eval_author
```

`tests/install.json` pins the `author` slot and the 3 documents an install
creates, reinstalls without change and removes.

## Operational history

None recorded yet.
