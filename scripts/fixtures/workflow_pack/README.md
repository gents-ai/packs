# Workflow fixture pack

This assets pack exercises `scripts/test-pack.sh`: the workflow index check
(`scripts/rhai_meta.py check-index`) and the `replay` case kind.
It ships one trivial workflow, `hello-world`, and declares no inference slots.

## Configuration

None: this is an assets pack with no inference slots and no runtime
configuration of its own.

## Usage

Run `scripts/test-pack.sh scripts/fixtures/workflow_pack`.
Installation only materializes the files listed in `manifest.json`.
