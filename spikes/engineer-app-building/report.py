"""Print every recorded run as a table, oldest first."""
import json, os
root = os.path.dirname(os.path.abspath(__file__))
rows = [json.loads(line) for line in open(os.path.join(root, "runs.jsonl"))]
print("| Run | Task | Model | Skills | Result | Fresh tokens | Cached tokens | Tool calls | Help read | Bash | Source reads | Minutes | Bad or failed |")
print("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
for r in rows:
    print(f"| {r['run_id']} | {r['task']} | {r['model']} | {r['skills']} | {r['status']} | {r['prompt_tokens']-r['cached_tokens']+r['completion_tokens']:,} | {r['cached_tokens']:,} | {r['tool_calls']} | {r.get('instruction_chars','')} | {r['bash_calls']} | {r['source_reads']} | {r['wall_minutes']} | {'; '.join(r['bad']) or 'none'} |")
