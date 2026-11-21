"""Record one app-building run: the same measurements every time.

Appends a row to runs.jsonl beside this script and keeps the run's trace, so
results can be compared as the skills and the platform change. Read-only
against the node.

  python3 -I record_run.py --home H --run-id ID --task dashboard --model M \
      --skills pack_builder@0.1.0 --binary rc1 --status working|not_working|in_progress \
      [--session S] [--bad "note"]... [--note "text"]
"""
import argparse
import collections
import datetime
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
SOURCE = re.compile(r"gents-scan-snapshot|crates/gents|gents-ai/gents|\.cargo/git")


def run(*argv):
    return subprocess.run(argv, capture_output=True, text=True).stdout


def query(home, collection, fields, session=None, order=None):
    rows, offset = [], 0
    while True:
        argv = ["gents", "query", "find", "--home", home, "--collection", collection,
                "--limit", "1000", "--offset", str(offset)]
        for field in fields:
            argv += ["--field", field]
        if session:
            argv += ["--filter", json.dumps({"session_id": {"_eq": session}})]
        if order:
            argv += ["--order", json.dumps([{order: "ASC"}])]
        page = json.loads(run(*argv) or '{"results":[]}').get("results", [])
        rows += page
        if len(page) < 1000:
            return rows
        offset += 1000


def count(home, collection):
    out = run("gents", "query", "count", "--home", home, "--collection", collection)
    try:
        return json.loads(out)["total_count"]
    except (ValueError, KeyError):
        return None


def command(row):
    raw = row.get("raw_arguments") or ""
    try:
        parsed = json.loads(raw)
        return parsed.get("command") or raw if isinstance(parsed, dict) else raw
    except ValueError:
        return raw


def main():
    parser = argparse.ArgumentParser()
    for name in ("home", "run-id", "task", "model", "skills", "binary", "status"):
        parser.add_argument(f"--{name}", required=True)
    parser.add_argument("--session")
    parser.add_argument("--node-counts", action="store_true")
    parser.add_argument("--bad", action="append", default=[])
    parser.add_argument("--note", default="")
    args = parser.parse_args()
    home = os.path.expanduser(args.home)

    run_dir = os.path.join(ROOT, args.run_id)
    os.makedirs(run_dir, exist_ok=True)
    trace_path = os.path.join(run_dir, "trace.jsonl")
    argv = ["gents", "trace", "export", "--home", home, "--limit", "20000", "--output-file", trace_path]
    if args.session:
        argv += ["--session-id", args.session]
    subprocess.run(argv, capture_output=True)
    trace = sorted((json.loads(line) for line in open(trace_path)), key=lambda r: r.get("started_at") or "")

    tools = collections.Counter(r.get("native_or_meta_tool") for r in trace)
    bash = [command(r) for r in trace if "bash" in str(r.get("native_or_meta_tool"))]
    config = [command(r) for r in trace if r.get("native_or_meta_tool") == "config"]
    pack_ops = collections.Counter(m.group(1) for c in bash for m in re.finditer(r"gents pack (\w+)", c))
    for c in config:
        m = re.search(r'"argv":\s*\[\s*"pack",\s*"(\w+)"', c)
        if m:
            pack_ops[m.group(1)] += 1

    calls = query(home, "InferenceCall", ["prompt_tokens", "completion_tokens", "cached_input_tokens"], args.session)
    prompt = sum(c.get("prompt_tokens") or 0 for c in calls)
    completion = sum(c.get("completion_tokens") or 0 for c in calls)
    cached = sum(c.get("cached_input_tokens") or 0 for c in calls)

    requests = query(home, "AgentRequest", ["runtime_source_kind", "caused_by_trigger_id", "input"], args.session)
    human = sum(1 for r in requests if '"source": "user"' in json.dumps(r.get("input")) or not r.get("input"))

    timed = [r for r in trace if r.get("started_at")]
    now = datetime.datetime.now(datetime.timezone.utc).isoformat()
    start = timed[0]["started_at"] if timed else now
    end = max((r.get("completed_at") or r["started_at"] for r in timed), default=now)
    parse = lambda t: datetime.datetime.fromisoformat(t.replace("Z", "+00:00"))
    def is_help(r):
        a = r.get("raw_arguments") or ""
        return r.get("native_or_meta_tool") == "load_skill" or '"help"' in a or "--help" in a
    instruction_chars = sum(len(str(r.get("tool_result") or "")) for r in trace if is_help(r))
    output_chars = sum(len(str(r.get("tool_result") or "")) for r in trace)
    row = {
        "run_id": args.run_id,
        "recorded_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "task": args.task, "model": args.model, "skills": args.skills, "binary": args.binary,
        "status": args.status,
        "total_tokens": prompt + completion, "prompt_tokens": prompt, "cached_tokens": cached,
        "completion_tokens": completion, "inference_calls": len(calls),
        "tool_calls": len(trace), "failed_tool_calls": sum(1 for r in trace if r.get("tool_failure_class")),
        "bash_calls": len(bash), "config_calls": len(config),
        "source_reads": sum(1 for c in bash if SOURCE.search(c)),
        "skills_loaded": tools.get("load_skill", 0),
        "pack_ops": dict(pack_ops), "human_messages": human,
        "wall_minutes": round((parse(end) - parse(start)).total_seconds() / 60, 1),
        "sessions": count(home, "AgentSession") if args.node_counts or not args.session else None,
        "trigger_fires": count(home, "TriggerFire") if args.node_counts or not args.session else None,
        "instruction_chars": instruction_chars, "tool_output_chars": output_chars,
        "bad": [b for b in args.bad if b], "note": args.note,
    }
    with open(os.path.join(ROOT, "runs.jsonl"), "a") as ledger:
        ledger.write(json.dumps(row) + "\n")
    print(json.dumps(row, indent=1))
    print(f"| {row['run_id']} | {row['task']} | {row['model']} | {row['skills']} | {row['status']} | "
          f"{row['total_tokens']:,} | {row['tool_calls']} | {row['bash_calls']} | {row['source_reads']} | "
          f"{row['wall_minutes']} | {'; '.join(row['bad']) or 'none'} |")


if __name__ == "__main__":
    sys.exit(main())
