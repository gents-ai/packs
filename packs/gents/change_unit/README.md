# Change unit pack

This pack runs one operator-authored `ChangeUnitWork` through a native Gents workspace callback, an exact-path writer, an independent read-only reviewer, and a host-owned serial integration. The review and closure preserve the writer receipt ID and seal hash. Rejected reviews produce a terminal result and never wake integration.

## Install

```sh
GENTS_CHANGE_UNIT_ROOT=/absolute/path/to/repository \
  gents pack install ./packs/gents/change_unit --home <home> \
    --inference-slot writer=<profile-id> \
    --inference-slot reviewer=<profile-id> \
    --inference-slot integrator=<profile-id>
```

`GENTS_CHANGE_UNIT_ROOT` is required: it becomes the `change-unit-repository` `RepositoryPlacement` and the root of the writer and reviewer tools, and must be a clean Git checkout. The placement ID is fixed, so one home serves one repository.

## Submitting work

Create a work-unit document with the pinned repository ID and base commit, a unique work-unit ID beginning `change-unit:`, a unique branch, a concise title and instructions, and an `owned_files` JSON string containing the complete relative file-path array. The prefix isolates this pack's event subscriptions from other workspace receipts. `handoff_id` and `caused_by_correlation` must equal the work-unit ID so later receipts, outcomes and results correlate to the unit. Supply the originating `reply_session_id` and a positive `attempt` so terminal outcomes retain supervision provenance. Retry bounds belong to the submitting supervisor; each retry needs a fresh unit/workspace identity.

```sh
gents document create ChangeUnitWork --home <home> --json \
  '{"work_unit_id":"change-unit:change-001","repository_id":"change-unit-repository","base_sha":"<commit-sha>","branch":"change-001","title":"...","instructions":"...","owned_files":"[\"packs/example/file.md\"]","caused_by_correlation":"change-unit:change-001","handoff_id":"change-unit:change-001","reply_session_id":"<supervisor-session>","attempt":1,"status":"ready"}'
```

The writer workspace uses the plain `git_worktree` adapter, which copies no build artifacts. Set `adapter` to `make_worktree` to clone build artifacts as well, and `clone_artifacts` to the relative paths to clone; without `clone_artifacts`, `make_worktree` clones the runtime's defaults.

## Results

Each unit ends with at most one `ChangeUnitResult`, whose `result_id` is the work-unit ID. The runtime fills its identity, routing (`handoff_id`, `reply_session_id`) and `status` from the triggering document:

- `completed`: the host integrated the accepted change; the result carries the closure, writer and integrator receipt references and `head_sha`.
- `rejected`: the review rejected the change; the result carries the closure references.
- `failed` or `interrupted`: the writer, reviewer or integrator stage ended without completing; `failure_stage` names its trigger and `failed_request_id` its request. A reviewer that wrote its closure before ending leaves the result to that closure.

Create a new work unit to try a revised request. The pack does not create a planning or repository-maintenance factory. Workspace creation failures produce no result; they remain visible in Gents' durable callback invocation and request/workspace records.

The pack keeps atomic tasks in fresh sessions. Configure an existing supervisor-owned Task and Trigger separately to deliver terminal results into the originating requester-scoped session; durable results alone do not wake a supervisor.
