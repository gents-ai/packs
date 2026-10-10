Job {{ doc.job_id }} (arm {{ doc.arm }}, suite {{ doc.suite }}) fired by
trigger {{ event.trigger_id }} on {{ event.source_collection }} doc
{{ event.source_doc_id }} at {{ ctx.now }}, handled by agent
{{ node.agent_id }} on {{ node.node_did }}.

{{ doc.prompt }}

After answering, call write_experiment_finding (unique finding_id, content,
stage="stage1"). After that write succeeds, call update_goal with
status="complete"; the Task's durable goal is the terminal condition for this
stage.
