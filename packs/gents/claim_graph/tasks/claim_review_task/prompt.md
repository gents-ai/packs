Review research request {{ doc.request_id }} in graph {{ doc.graph_id }}; its researcher filed receipt {{ doc.receipt_id }}.

- request_id: {{ doc.request_id }}
- graph_id: {{ doc.graph_id }}
- claims_admitted: {{ doc.claims_admitted }}
- claims_memory: {{ doc.claims_memory }}
- open_questions: {{ doc.open_questions }}
- now: {{ ctx.now }} (today is {{ ctx.now[:10] }})

Researcher's summary:

{{ doc.summary }}

Follow the review loop. The claims to review are those whose `batch_id` starts with `{{ doc.request_id }}-`. Name every batch `{{ doc.request_id }}-review-<n>`.
