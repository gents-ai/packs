---
name: package-pack
description: Load when making an app installable or changing an installed pack. Layout, check, install, versions.
---

# Package a pack

A pack is a directory with `manifest.json`. Only files the manifest declares travel with it. Start with `pack scaffold NAME`, adding `--page true` for a page (`config help pack`). `pack build` then `pack install ./DIR` installs it. You can install any pack yourself.

## What goes where

| Part | Location | Declared under |
| --- | --- | --- |
| Collections | `schemas/*.graphql` | `assets` |
| Agents, tools, surfaces, tasks, triggers | `pack_config.json` | `config` |
| Prompts | Markdown that `pack_config.json` names | `assets` |
| Page | `ux/<name>/plugin.js` and `plugin.css` | `ux` and `assets` |
| Compiled code | `plugins/` | `plugins` and `assets` |

The check and install enforce:

- A plugin's data collections are declared in a `schemas/*.graphql` file in the pack.
- A page's surfaces and tools are listed under `ux[].data`.
- Install takes inference slots as `--inference-slot NAME=VALUE`.
- A plugin in Tools is named `<namespace>/<plugin>`, such as `gents/rules`. The pack name isn't part of it.
- `pack check` takes only an optional directory, and no options.

If your tools can't create a part, say so before building.

## The loop

1. Make one change.
2. Run the pack check and fix every finding.
3. Install.
4. Open a fresh request to exercise it. Changes apply to later requests only.
5. Check the actual behavior.

Don't edit the installed copy or delete cached pack files. Fix the source and reinstall.

Installing a plugin doesn't select it: add it to the Tools of each agent that needs it, then use a fresh request. Bump `version` on each meaningful change; many builds under one version hide what is running.

Before you finish, reinstall on a clean state if you can, and recheck ACCEPTANCE.md.
