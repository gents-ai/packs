Independently verify book run {{ doc.run_id }}.
Title: {{ doc.title }}. Author: {{ doc.author }}. Language: {{ doc.language }}.
Source manifest: {{ doc.sources_json }}
Proposed starts: {{ doc.entries_json }}
Notes: {{ doc.review_notes }}

Check actual pages and fix the outline before writing the structure proposal.

{% if doc.feedback %}Native validation rejected the previous proposal: {{ doc.feedback }}
Repair the reported issue using the supplied proposal and page evidence. Preserve
verified boundaries. Submit a corrected proposal; do not repeat the rejected JSON.
{% endif %}
