The host integrated work unit {{ doc.source_handoff_id }} in request {{ doc.request_id }}.

Read the unit's closure and the integrator receipt this request produced. Write one ChangeUnitResult copying closure_id, implementation_id, review_id, workspace_id, writer_receipt_id and writer_seal_hash from the closure, integrator_receipt_id, integrator_seal_hash and head_sha from the receipt's receipt_id, seal_hash and head_sha, attempt {{ doc.attempt }} as a JSON integer when present, and a one-line summary.
