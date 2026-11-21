# Skills audit 1

Nine skills from `packs-spike-pack-builder/packs/gents/pack_builder/skills`, rewritten in this directory. Nothing else was changed. The installed copies in the dashboard-with-skills home are byte-identical to the source, so the audit covers the current versions.

## Character counts

Counted as characters of the whole file, frontmatter included.

| Skill | Before | After | Cut | Description before | Description after |
| --- | ---: | ---: | ---: | ---: | ---: |
| plan-work | 3038 | 2278 | 25% | 265 | 94 |
| model-data | 2177 | 1677 | 23% | 182 | 91 |
| connect-data | 1822 | 1410 | 23% | 162 | 97 |
| design-page | 3521 | 2187 | 38% | 207 | 93 |
| automate | 2247 | 1657 | 26% | 197 | 103 |
| control-access | 1611 | 1069 | 34% | 185 | 101 |
| package-pack | 1946 | 1671 | 14% | 195 | 100 |
| write-plugin | 2816 | 1471 | 48% | 249 | 102 |
| diagnose-node | 2020 | 1336 | 34% | 226 | 97 |
| **Total** | **21198** | **14756** | **30.4%** | | |

Further cuts, if pushed: the design-page block list could go to names only, with props left to the SDK reference. The package-pack location table could move to `config help pack`. Plan-work's section 3 table could become one sentence.

## What the runs covered

- **Risk baseline, no skills** (`results/risk-20261104-baseline/trace.jsonl`, 2,159 calls): 1,713 bash calls, 309 of them reading Gents source, 18 `config` errors out of 218. Most of the user's complaints come from this run. Each one maps to a line in a skill (table below).
- **Dashboard, no skills** (`dash/`, 228 calls): 175 bash calls, source reading from a repo snapshot, no ACCEPTANCE.md, and four pack check and install errors in `config` (a missing schema declaration, `--inference-slot` form, `pack check` taking no options, and an unknown store digest).
- **Dashboard, with skills** (`dash-skills/`, partial): `turn1.log` runs to line 206 and was last written at 10:51. `trace_now.jsonl` stops at 10:29, so it is partial. The run loaded seven skills between 17:57 and 18:05, all before any build. It then ran about 100 calls of discovery, including `strings` on the installed gents binary six or more times, before writing a complete ACCEPTANCE.md as its last action. It did not load `write-plugin` or `diagnose-node`, and no page had been written when the log ended. Treat this as partial evidence.

## Contradictions and overlaps found

1. **Path gap.** The build list in `plan-work` never named `write-plugin`. It now does.
2. **Load timing.** `plan-work` said to load each skill "when you reach its step", but the run loaded five later skills up front. Now it says to load only the current step's skill.
3. **Shell assumptions.** `package-pack` (scaffold, check, install loop), `write-plugin` (`gents plugin run`, "run the pack's tests") and `design-page` (page lint) all assume a CLI. The eval runs with no shell, and the spec says compiler-backed plugin builds and tests need the operator CLI. Not resolvable inside skills. `write-plugin` now says to report a plugin as not tested if tests can't run.
4. **Lint versus preview.** The old `design-page` said to lint. The next test has `config ux preview`, which renders and reports problems. The skill now names the preview. `gents ux lint` still exists in the trace help, so if the next test still has it, add it back.
5. **`trigger explain` does not exist yet.** The old `diagnose-node` said "use it if this node has it". The spec lists it as unbuilt. Removed.
6. **Repeats of the Engineer prompt.** The prompt already says prompts and skills grant nothing, that a task can end with a reply, that install does not select a plugin, and that config reads verify setup. The skills repeated these. Each now appears once, in the most relevant skill: `control-access` no longer restates the grant rule, `automate` no longer restates the reply rule, and `package-pack` keeps the plugin-selection rule.
7. **Threshold wording.** `plan-work` says load `diagnose-node` after a step fails twice. `diagnose-node` says stop and ask after three attempts. These are a deliberate escalation, so both stay, stated in the same terms.
8. **Old design-page token list and primitives.** It listed only primitives and no tables, stats or charts, so a model would hand-build them. That is the 40 KB page problem in the packs issue. Replaced with the SDK blocks.
9. **Duplicate `useQuery` note.** The old `design-page` repeated the "ctx comes first" point in two places. Now one line.

## Instructions the runs show were ignored, and what changed

| Instruction | What the run did | Change |
| --- | --- | --- |
| Write ACCEPTANCE.md before building | Written last, after about 100 calls. | `plan-work` now opens with "Before any build call, write ACCEPTANCE.md". Not yet proven. |
| Load each skill when you reach its step | Seven loaded before any build. | "Load only the current step's skill." Not yet proven. |
| Use tools and help before source | Ran `strings` on the installed binary, and in the baseline grepped the home directory (the user objected). | `plan-work` now names both moves. The Engineer prompt already says this in general. |
| Enable triggers one at a time | Not reached in the dashboard run. The baseline ignored it and produced about 16,000 fires. | No new wording beyond the existing step list. Cannot tell from the runs. |
| Lint or check the page | Not reached. | Replaced with the preview command. |
| Describe a restriction only after seeing it refuse | The dashboard ACCEPTANCE.md marked teammate access as not checked. | Followed. Kept. |

The dashboard ACCEPTANCE.md in the skills run was a good file: it separated "not claimed" from the rest. Nothing in it needs changing.

## Failures no skill can fix

1. **Session spam** (about 16,000 fires in the baseline). `automate` covers routing and loop checks, but the platform gaps are outside any skill. A trigger cannot create or get a session by key, so a per-game session is not possible. `agent_new` issues UUIDs. There is no per-trigger fire budget. There is no pack check for a task that writes its own source collection. These come from the spec's "needed in gents-ai/gents" list.
2. **Subagents.** A fresh Engineer's `engineer_tools` enables the agents group with no targets, so `agent_new` has nothing to start. The user asked for subagents and this is why they could not be started.
3. **Page scaffolding.** `config` cannot scaffold a page, and `gents pack add` cannot add a page to an existing pack. `package-pack` now tells the model to report a part it cannot create.
4. **Access enforcement.** Pack check does not require a declared access. There is no pack-level access declaration. Sharing and enrollment are on hold for gents-ai/gents#3538. `control-access` can only say what is not enforceable.
5. **Plugin tests.** Building and testing need the operator CLI, so the no-shell eval cannot run them.
6. **Config syntax errors.** Examples from the baseline: an event-source filter that must be an object literal, a `tools get` given a name rather than a document ID, `trigger update --enable` without a value, and an agent create that needs a published inference profile. Only tool help can prevent these. Skills now point at help.
7. **Unexplained page error.** The baseline's `n.onEvent is not a function` was never diagnosed in the traces. No skill rule can be tied to it, so none was added.
8. **Source reading.** Skills say to use help first, but the baseline read source 309 times. This is only partly a skill problem. The Engineer prompt makes the same request, and the thinness of `config help` is the other half.
9. **Reachability of the SDK reference.** `design-page` relies on the SDK reference for block props. If the Engineer cannot read that file, the skill points at nothing. This is a tooling question.

## Top three changes, by expected effect on time to a working build

1. **`plan-work`: ACCEPTANCE first, one skill at a time, and no binary or home-directory discovery.** The dashboard run spent about 100 calls on discovery and loading before building. This is the largest source of wasted calls in the evidence.
2. **`design-page`: name the SDK blocks, the preview command and the token rule.** The old skill knew only primitives, so a model would hand-build tables and charts. The token rule (never `--background` or `--foreground`) is what caused the dark-text complaint in the baseline. Expected to reduce page iterations.
3. **`package-pack`: the install and check rules real runs hit.** The no-skill dashboard run had four such errors: a plugin collection not declared in a schema file, the `--inference-slot NAME=VALUE` form, `pack check` taking no options, and a missing store digest. Each cost a round trip. The rules are now in the skill, so the first install should succeed.

Next best: `write-plugin`'s `data_calls` shape (cost several rounds in the baseline) and `automate`'s session rule (dropped fires).

## Claims removed or left unverified

Removed because I could not verify them against the source, the runs or the spec:

- `plugin-tool` template name.
- `@branchable` "cannot be changed later".
- Schema tool cannot refresh a materialized view.
- Plugin input "not piped stdin". The help says input is delivered on stdin. I kept the command and dropped the clause.
- "One round asks for one kind of host request".
- `ctx` must come first for "is not a function".
- Concurrency: "parallel is the default", and the meanings of values other than `serial`. Named `serial` only. `queued_serial` and `latest_only` also appear in the runs.
- A session template must use the task's agent.
- "Every file in `assets` exists".
- `gents ux lint` (replaced by preview).
- `pack new --ux` as the scaffold path.

Kept but not verified against source: the package layout table, `ux[].data` listing rule, `plugins/<name>/tests/*.json`, the refusal output shape (a pattern, not a Gents convention), trigger "status and last error" field names, and "a node a person joins can expose more than one app's data" (from the old skill and consistent with the spec).

Verified: `data_calls` read limit of 256 and 64 rounds, `writes` limit of 100, `data_results` key name, `--inference-slot NAME=VALUE`, `pack check` takes only a directory, `plugin run --input`, `schema help`, `config agent get`, `config help automation`, `serial` concurrency value, `schema help collections policy`, `p2p help scope`, and `pack new --kind documents`.

## Failure each skill prevents

| Skill | Failure it prevents | Evidence |
| --- | --- | --- |
| plan-work | Build starts before requirements are written; discovery loops; scripts stand in for triggers | Dashboard run (ACCEPTANCE last); Risk run (outside script driving the game) |
| model-data | Every move wakes every player because the turn is not a field on the event | Risk run |
| connect-data | Imports that write duplicates or wipe data when a file is bad | Spec's import requirements |
| design-page | Invisible dark text; hand-built tables and charts; list running off screen | Risk run complaints; design-page token rule |
| automate | Session spam and self-firing loops | Risk run (about 16,000 fires) |
| control-access | Promising teammate-only access the node cannot enforce | Dashboard requirement; gents-ai/gents#3538 |
| package-pack | Install and check errors; version churn | Dashboard run errors; 105 builds of one version in the Risk run |
| write-plugin | Costly plugin round trips from wrong `data_calls` shape | Risk run |
| diagnose-node | Restart requests and binary reads instead of checking the chain | Risk run (309 source reads, unproven restart theory) |

## Renames and merges

- **No renames applied.** The current names are consistent, and the next test should compare like with like. If the maintainer wants a change, `automate` is the odd one out: it is a verb while the rest are nouns or verb phrases. A candidate is `run-on-events`.
- **No merges proposed.** `control-access` is short, but the eval requires access declarations, so it stays. Merge it into `package-pack` only if the test shows it is never loaded.
- `add-node` and `span-nodes` from packs#54 are not in this set, so they are out of scope here.

## Unsure

- Whether the Engineer can read `sdk_reference.md`. `design-page` depends on it.
- Whether the next test has `gents ux lint` as well as `config ux preview`.
- Whether the exact argv `["ux","preview","<plugin id>"]` matches the real command. I used the brief's form.
- Whether the no-shell eval makes `gents` commands unavailable. Several skills still name them.
- Whether the dashboard-with-skills run has finished. My counts are as of its files at 10:51.

I did not run any `gents` command, start or stop any process, or write outside this directory. I read the traces and logs, the installed skill copies and the workspace listing, and ran read-only searches of the gents repo and `gh issue view 54`.
