# rhai_workflows_web

## What it is

This pack adds two `wf-*` personas that a Rhai workflow from `gents/rhai_workflows` can run its agent steps on, both searching and reading the web with `gents/web_search` and `gents/web_fetch` from the `gents/web` pack:

- `wf-research` has no file or shell access.
- `wf-read-only-web` reads files and runs read-only shell commands without network, so what it reads can reach the web; the operator opts in by wiring it as a target.

Neither persona gets a crawl or browser tool, and no external service is needed.
They live apart from `gents/rhai_workflows` so that installing the workflow engine never references a plugin it does not ship.

## Install

The pack depends on `gents/rhai_workflows` and `gents/web`, and the operator installs it explicitly:

```
gents pack install ./packs/gents/rhai_workflows_web --grant-authority \
  --inference-slot wf_research=PROFILE_ID \
  --inference-slot wf_read_only_web=PROFILE_ID \
  --inference-slot web_researcher=PROFILE_ID \
  --inference-slot wf_read_only=PROFILE_ID \
  --inference-slot wf_read_write=PROFILE_ID \
  --inference-slot wf_execute=PROFILE_ID \
  --inference-slot wf_all=PROFILE_ID
```

`--grant-authority` records the install as an Operator action and covers every `gents/web` plugin, including the outbound HTTP the web tools need, as well as the `gents/rhai_workflows` engine's limits.
`web_researcher` is the `gents/web` pack's own slot, and the four `wf_*` slots after it belong to `gents/rhai_workflows`; the install asks for every dependency's slots, even when that dependency is already installed.

A documents pack with dependencies needs gents 12d96f21b or later; an older gents refuses the install.
Test the pack against such a build with `GENTS=<path to that gents> scripts/test-pack.sh packs/gents/rhai_workflows_web`.

## Wiring

Let a workflow use these personas as child targets the same way as the engine's own:

```
gents pack wire gents/workflow --agent <agent> --targets-from gents/rhai_workflows --targets-from gents/rhai_workflows_web
```

Leave out `--targets-from gents/rhai_workflows` to give the workflow only the web personas.
