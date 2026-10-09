Section: {{ doc.title }}
Edition: {{ doc.run_id }}
Chunk: {{ doc.chunk_ref }}

Review these complete, bounded blocks. Their source offsets are immutable:
{{ doc.blocks_json }}

{% if doc.source_pages_json %}Source page coordinates for optional visual inspection: {{ doc.source_pages_json }}
Use get_page_ocr or load_page_image when an OCR fragment needs source evidence. page_num is scan_page from these coordinates. Visual inspection returns the bound vision model’s observations.
{% endif %}

Return only supported local corrections using write_shelf_polish.

{% if doc.feedback %}Previous edits were rejected: {{ doc.feedback }}
Correct the exact-match problem using the original blocks above. If a correction cannot be located precisely, omit it rather than guessing.{% endif %}
