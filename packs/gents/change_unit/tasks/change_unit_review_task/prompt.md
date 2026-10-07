Review work unit {{ doc.work_unit_id }} in sealed workspace {{ doc.workspace_id }}, writer receipt {{ doc.receipt_id }}, seal {{ doc.seal_hash }}.

The receipt's changed paths are {{ doc.changed_files }}. Read the unit's ChangeUnitWork and ChangeUnitImplementation. Require every receipt path to be in `owned_files` (owned paths need not all change) and the implementation's `changed_files` to equal the receipt's set exactly. Inspect every changed file and the diff from the pinned base against the instructions and tests.

Write one ChangeUnitReview with a unique review_id, the implementation_id, verdict `accepted` or `rejected`, findings and a concise summary. Then write one ChangeUnitClosure with a unique closure_id, the same implementation_id and review_id, and status equal to the verdict. Copy the ChangeUnitWork's attempt into both as a JSON integer. The runtime fills the unit, workspace, receipt and seal fields.
