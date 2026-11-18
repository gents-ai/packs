# pr_explorer_ux

PR Explorer's page. An `assets` pack with one UX plugin
(`ux/pr_explorer/plugin.js`): a **PR Explorer** nav row, the page it opens,
and a `::pr-explorer{analysis="…"}` chip the pr-explorer agent ends its reply
with. The data half is `../pr_explorer`: install it on the node first.

## The page

```
┌ PR Explorer ─ [repo ▾] [analysis ▾] ─ [Open on GitHub] [Refresh] [New analysis] ┐
│ ▾ Schema change  3 files │ M src/schema.rs  +8 −1                        │
│   summary…               │ ⚠ Truncated: … (when the stored diff was cut) │
│   M src/schema.rs +8 −1  │  old  new                                     │
│   A migrations/003.sql   │   10   10   context                           │
│ ▸ Tests          2 files │   11       -removed                          │
│ ▸ Unassigned (if any)    │        11  +added                            │
└──────────────────────────┴───────────────────────────────────────────────┘
```

- Units come from `pr_list_units`, files from `pr_list_files` (without the
  diff); a file's diff is fetched when it is selected. Files no unit claims
  show under **Unassigned**; a path a second unit claims is flagged `dup`,
  one not in the change `missing`.
- Diff lines are React text in a table, never HTML; 5,000 lines are drawn
  at a time with **Show more**.
- With no analysis on the node, the page offers a form: a GitHub PR link
  (`https://github.com/<owner>/<repo>/pull/<n>` or `<owner>/<repo>#<n>`), or a
  local range (repo, base, head). **Analyze** sends "Review <PR URL> with PR
  Explorer." or "Analyze <base>...<head> in <repo> with PR Explorer." as your
  next turn in the **selected session**, only on the click: select a session
  with the PR Explorer agent first. **New analysis** shows the same form.
- The analysis picker labels a PR analysis `owner/repo #n`; **Open on
  GitHub** opens its github.com URL, rebuilt from validated parts (never the
  stored text).
- The page fills its frame. On gents desktop builds that still draw plugin
  pages in the centred reading column, it fits that column; a desktop change
  that gives plugin pages the whole main pane is proposed separately.

The manifest declares only the three query tools; the page writes nothing.

## Install

```sh
gents pack install gents/pr_explorer_ux          # from the registry
gents pack install ./packs/gents/pr_explorer_ux  # or from a checkout
```

Then in the desktop: Agent ▸ UX Plugins ▸ Reload, and the PR Explorer row
appears in the nav.
