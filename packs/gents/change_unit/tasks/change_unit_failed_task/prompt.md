Stage {{ doc.trigger_id }} of work unit {{ doc.source_handoff_id }} ended `{{ doc.terminal_state }}` in request {{ doc.request_id }}.

When the stage is `change-unit-review`, first read the unit's closure. If one exists, the reviewer recorded its verdict before ending and that closure records the unit's result, so finish without writing. Otherwise write one ChangeUnitResult with attempt {{ doc.attempt }} as a JSON integer and a one-line summary.
