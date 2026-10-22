# rhai_workflows

## What it is

This pack gives an agent the `workflow` tool.
The tool runs Rhai workflow scripts that orchestrate background agents.
A script is deterministic: the engine replays it from the start on each step and answers every host call from a journal, so a paused or interrupted run resumes where it stopped.
Call the tool with `["help"]` for the verbs, the language, the host functions and the error codes.

## Install

```
gents pack install <source> --grant-authority
```

`--grant-authority` records the install as an Operator action.
The manifest declares limits above the plugin defaults, and only an Operator may grant those.
An engine installed before the host supports job tools must be reinstalled by the operator.

## Wiring

Wire the tool to the agents that may start workflows:

```
gents pack wire gents/workflow --agent <agent> --targets-from <pack>
```

`--wire` on install does the same.
`--targets-from` names the pack whose agents become the workflow's child targets, and it also trusts that pack.
To scaffold a project with the engine from the start, run `gents init --workflows`.

## Default policy

Until a pack is trusted, the default policy block runs nothing: no workflow target is usable.
This is deliberate, so installing the engine alone never lets a script reach an agent.
Trust a pack with the wiring step above.

## Personas

Four `wf-*` personas are the targets a workflow runs its agent steps on: `wf-read-only`, `wf-read-write`, `wf-execute` and `wf-all`.
They differ in file access, shell and network, from read-only to everything.
Two personas that research the web, `wf-research` and `wf-read-only-web`, come from the separate `gents/rhai_workflows_web` pack, which the operator installs explicitly along with `gents/web`; this pack references no plugin but its own.

## Compatibility with Grok scripts

The engine's differential test runs a gents-written suite of scripts through two optimizer levels and compares the transcripts.
Set `GROK_CORPUS_DIR` to a directory of scripts and the test adds them to the suite.
The corpus is read from the environment and is never committed.
