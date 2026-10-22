# Workflow lifecycle

Where a script comes from
- pack: a `workflows` asset of an installed pack. Name it `pack:<ns>/<pack>/<name>`, or give the bare name.
- project: `*.rhai` files in `<cwd>/.gents/workflows` or `<cwd>/.grok/workflows`, used as `project:<name>`. The tier is on only when the session's folder is under the operator's `project_roots`; `help doctor` says which.
- kept: your own saved scripts, used as `kept:<name>` or `kept:<name>@<version>`.
- inline: the `script` argument. path: the `path` argument, one file read once at the start.
- `sha256:<script_id>` runs a script this node already stored.
A bare name resolves in the order pack, project, kept. Two packs holding one name is refused; qualify it.

keep
- `workflow keep <key|handle|script_id> [--name n] [--description d] [--when-to-use w] [--replace]` saves a checked script under its `meta.name`; each keep adds a version.
- A numbered handle such as `x-2` needs `--name`. A name already kept needs `--replace`. A name a pack holds is always refused.

retire
- `workflow retire <name>` adds a retired version, so the name stops resolving. Runs already started keep their source.

export
- `workflow export <name> [--namespace ns] [--pack-name p]` returns the files of an assets pack and its digest. Write them to a folder, then `gents pack install <folder> --expect-digest sha256:<hex>`.
- A script stored with the license `xai-bundled-local-only` is refused: it never leaves the node. Nothing in v1 writes that license, so export does not check whether a script came from another product.

Promotion to installed
- Kept, project, inline and path scripts run with origin `dynamic`: the operator's policy decides whether they may start.
- A script runs as `installed` only from a pack the operator installed and listed in `trusted_packs`. Exporting then installing gives the pack; adding it to `trusted_packs` is the operator's edit, and you cannot make it.
