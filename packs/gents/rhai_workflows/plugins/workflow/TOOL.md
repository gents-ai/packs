Runs Rhai workflow scripts that orchestrate background agents.
argv is a verb and its operands: list, show <name>, check, run <name>,
status|result|stop|pause|resume|answer <key|handle>, keep, export,
retire. Start with ["help"]; ["help","<verb>"] explains one verb.
`run` returns a key and handle at once. The result arrives later as a
message: end your turn instead of polling. Put a new script's source in `script`, a file
in `path`, and the workflow's inputs in `args`. A saved workflow runs by
name with no `script`; `run <name> --budget <n>` caps its agents.
