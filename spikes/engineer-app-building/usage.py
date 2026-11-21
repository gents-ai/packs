"""usage.py RUN...: which tools, skills and config commands each run used."""
import json, collections, sys
for run in sys.argv[1:]:
    c = collections.Counter(); sk = []; cfg = collections.Counter(); fails = collections.Counter()
    for l in open(f'{run}/trace.jsonl'):
        r = json.loads(l); n = r.get('native_or_meta_tool'); c[n] += 1
        try: a = json.loads(r.get('raw_arguments') or '{}')
        except Exception: a = {}
        if not isinstance(a, dict): a = {}
        if n == 'load_skill': sk.append(a.get('skill_id') or a.get('name') or str(a)[:30])
        if n in ('config', 'schema'):
            argv = a.get('argv') or []
            key = n[0] + ':' + ' '.join(map(str, argv[:3 if argv[:1] in (['help'], ['collection']) or 'preview' in argv[:2] else 2]))
            cfg[key] += 1
            if r.get('tool_result_ok') is False: fails[key] += 1
    print('==', run, sum(c.values())); print('  tools', c.most_common(9)); print('  skills', sk)
    print('  commands', cfg.most_common(22)); print('  failed', fails.most_common(8))
