---
name: plan-work
description: Load first when asked to build or automate an app for people. Sets the checks and build order.
---

# Plan the work

App: what the person asked for, by their name. Page: a screen in the desktop app. Plugin: compiled code, only for logic documents, queries and tools can't do. Pack: how an app is installed.

Before any build call, write `ACCEPTANCE.md` in your workspace.

## 1. Understand the job

Restate it in the person's words. Ask only what you can't discover, in one message: who uses it, where the data comes from and how often it changes, how fresh it must be, who must not see it.

## 2. Write ACCEPTANCE.md

One true-or-false line each, including at least one "must not happen":

- The urgent count on the page matches the file.
- Importing the same file twice changes nothing.
- A malformed file leaves yesterday's numbers in place and tells someone.

If a requirement can't be met on this node, say so now and offer the closest option.

## 3. Pick the smallest design

| Need | Design |
| --- | --- |
| An answer, once | Reply in chat |
| People look at data | Documents and a page |
| Data must stay current | A schedule or event with one small task |
| Each item needs judgment | An agent on a trigger |

Rules and validation go in queries or code. Agents are for judgment. A dashboard usually needs one scheduled task and no agent.

## 4. Build in this order

Load only the current step's skill: `use-database`, `connect-data` (only if data comes from outside this node), `design-page`, `automate`, `control-access`, `package-pack`. A step that needs compiled rules loads `write-plugin`, then you return here. On a long build, hand independent steps to a helper (`delegate-work`), and have one that did not build it check `ACCEPTANCE.md`.

For syntax, use `config help <resource>` or `--help`. Don't read the gents binary (no `strings`) or look for Gents source anywhere. Use tool help.

Don't wait by sleeping or polling for a person. If you need someone, file one mailbox item, end your turn and say what you're waiting for.

## 5. Prove it

Check each ACCEPTANCE line by running the thing the way it will really run. Config read-backs show setup; only a run shows it works. Never drive it with a script, manual call or helper agent and report that as the automation working. If the same step fails twice, stop changing things and load `diagnose-node`.

## 6. Hand over

Say what you built and how to open it. List each ACCEPTANCE line as passed, failed or not checked, and what the person still has to do.
