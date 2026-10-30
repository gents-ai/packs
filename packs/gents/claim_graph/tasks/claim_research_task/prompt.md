Research request {{ doc.request_id }} for graph {{ doc.graph_id }}.

- request_id: {{ doc.request_id }}
- graph_id: {{ doc.graph_id }}
- topic: {{ doc.topic }}
- brief: {{ doc.brief }}
- max_sources: {{ doc.max_sources }}
- create_graph: {{ doc.create_graph }}
- now: {{ ctx.now }} (today is {{ ctx.now[:10] }})

Follow the research loop. Begin with step 2: find `ClaimGraph` with `graph_id` `{{ doc.graph_id }}`. Name every batch `{{ doc.request_id }}-<section>-<n>`, every source request `{{ doc.request_id }}-s<NN>`, and the receipt `{{ doc.request_id }}-receipt`.
