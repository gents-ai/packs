# pr_explorer

The analysis half of PR Explorer. A `documents` pack: three collections,
the `pr_explorer` surface, the `pr_diff_split` plugin and the `pr-explorer`
agent. The page that shows an analysis is `../pr_explorer_ux`.

## Using it

1. Install both packs (see Install) and reload UX plugins in the desktop.
2. Start a session with the **PR Explorer** agent, with no workspace
   attached, and ask:
   - `Review https://github.com/<owner>/<repo>/pull/<n> with PR Explorer.`
   - or a local range: `Analyze main...my-branch in <repo> with PR Explorer.`
3. If several local clones match the PR's repository, it asks which one.
4. It replies with a summary and a `::pr-explorer{…}` chip; click it to open
   the page on that analysis (or open **PR Explorer** from the nav).

## Flow

1. Ask the **PR Explorer** agent (`pr-explorer`) to analyze a local range
   ("Analyze main...feature in gents") or a GitHub PR
   ("Review https://github.com/gents-ai/gents/pull/3189"). The page's Analyze button sends the
   same request to the selected session.
2. It resolves the range with `git rev-parse`/`merge-base` in the repository
   (for a PR, after reading it with `gh pr view` and fetching its commits),
   writes the patch to `<root>/.pr-explorer/patches/<analysis_id>.patch`
   with `git diff --output` (outside every repository, so linked worktrees
   work and no checkout changes), and calls `pr_diff_split`, which writes one
   `PrExplorerDiff` per changed file (diff bounded at 256 KiB, `truncated`
   when cut). Diff text never passes through the model. A last
   `pr_diff_split` call with `remove_patch` deletes the patch.
3. It groups the files into units (`pr_record_unit`), checks every file is in
   exactly one unit, records the `PrExplorerAnalysis` last, and replies with
   `::pr-explorer{analysis="…"}`, a chip that opens the page.

## Documents

| Collection | Written by | Fields |
|---|---|---|
| `PrExplorerDiff` | `pr_diff_split` | file_key (`<analysis_id>:<ordinal>`, unique), analysis_id, ordinal, path, old_path, change (A/M/D/R/C), binary, additions, deletions, diff, diff_bytes, truncated |
| `PrExplorerUnit` | agent, `pr_record_unit` | analysis_id, unit_id, ordinal, title, summary, files (JSON array as a string) |
| `PrExplorerAnalysis` | agent, `pr_record_analysis`, last | analysis_id, repository, pr_ref, base/head ref and sha, file_count, unit_count, created_at |

Append-only: a re-run is a new analysis. The surface's query tools are
`pr_list_analyses`, `pr_list_units` and `pr_list_files`; the page declares only
those three.

## Configuration

`GENTS_PR_EXPLORER_ROOT`: the folder holding your repositories (e.g.
`~/src`), set at install. It is the agent's workspace root; a request names a
repository relative to it, and a PR is matched to the clones among its
top-level folders. Patches live in `<root>/.pr-explorer/patches/`
only while a run splits them.

`pr_diff_split` binds only that folder: read-only while it splits (readable
without asking, as part of the session's working folder), read-write for the
final `remove_patch` call. Without a grant the node asks for consent at that
call; to skip the prompt, allow the folder once:

```sh
mkdir -p ~/src/.pr-explorer/patches
gents plugin dirs add ~/src/.pr-explorer/patches --access read_write
```

### Bash: network on, exact commands

Network is enabled (for `git fetch` and `gh`; core has no per-host switch).
Bash runs only these programs, matched token for token from the start of the
argument list; a shell string (`/bin/sh -lc …`) and `git -C`/`git -c` match
nothing and are refused, so push, checkout, reset, commit, config and ref
edits are impossible, not just discouraged:

`git rev-parse`, `git merge-base`, `git diff`, `git log`, `git show`,
`git ls-files`, `git remote get-url`, `git worktree list`,
`git fetch --no-tags origin`, `gh pr view`, `date -u`, `mkdir -p`.

Prompt-enforced only: what follows `git fetch --no-tags origin` (commit ids,
never a `src:dst` refspec) and where `--output`/`mkdir` write (inside the
root; the sandbox stops writes outside it).

### GitHub pull requests

Paste a link (`https://github.com/<owner>/<repo>/pull/<n>`) or
`<owner>/<repo>#<n>`. The agent reads the PR with `gh pr view` (open, closed
or merged; for a merged PR GitHub reports the base as it was at merge time),
finds the clones of `<owner>/<repo>` among the root's top-level folders by
their `origin`, asks which one when there are several (worktrees of one clone
share its commits, so the choice is per clone), and fetches the base and head
**by commit id**: commits land in that clone's object store, no branch or ref
is created, and nothing needs removing afterwards. github.com only; a PR with
no local clone is refused until fresh copies land.

Needs `gh auth login` on this machine (token in the keychain) and a git
credential helper that can read the repository. Run PR Explorer sessions
without a workspace attached: with one, core allows only read-only git and
the fetch is refused.

## Install

From the registry (once published from `master`):

```sh
GENTS_PR_EXPLORER_ROOT=~/src \
  gents pack install gents/pr_explorer --inference-slot analyzer=<profile>
gents pack install gents/pr_explorer_ux
```

From a checkout of this repo, use `./packs/gents/pr_explorer` and
`./packs/gents/pr_explorer_ux` instead. Any capable inference profile works
for `analyzer`; `gents pack install --preview` lists the ones on your node.

## Limits

- github.com pull requests only; other forges later (the analysis records
  the PR URL, so the page can tell them apart).
- A PR needs a local clone of its repository under the root; fresh scratch
  clones are not supported yet.
- Only committed changes are diffed: a local range runs from the merge base
  to the head commit.
- Each file's stored diff is capped at 256 KiB (`truncated` on the page), and
  an analysis stores at most 2,000 files.

## Layout

```
manifest.json, pack_config.json
schemas/                              the three collections
agent_behaviors/pr_explorer/          the agent's prompt
plugins/pr_diff_split/                Rust source, TOOL.md, cases (tests/)
plugins/pr_diff_split.afb             the built plugin (built by `gents pack build`; not committed)
tests/                                install and content cases
```
