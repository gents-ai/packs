Section: {{ doc.title }}
Edition: {{ doc.run_id }}
Chunk: {{ doc.chunk_ref }}

Review these complete, bounded blocks. Their source offsets are immutable:
{{ doc.blocks_json }}

Return only supported local corrections using write_shelf_polish.

{% if doc.feedback %}Previous edits were rejected: {{ doc.feedback }}
Correct the exact-match problem using the original blocks above. If a correction cannot be located precisely, omit it rather than guessing.{% endif %}
