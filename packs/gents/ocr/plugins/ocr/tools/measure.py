#!/usr/bin/env python3
"""Runs the built plugin under wasmtime and follows its cursor, reporting the
time and the peak memory of every call. Works on Linux and macOS (wasmtime and
python3 only); the memory limit is the pack's own, 1536 MiB.

Usage: tools/measure.py <ocr.wasm> <dir> '<input json>' [--calls N] [--mib 1536]
  <dir> is mapped to /data inside the sandbox, so the input names /data/<file>.
  --calls N stops after N calls (default: follow the cursor to the end).
"""
import json, os, subprocess, sys, tempfile, time


def main():
    args = sys.argv[1:]
    wasm, data, raw = args[0], args[1], args[2]
    calls = int(args[args.index("--calls") + 1]) if "--calls" in args else 0
    mib = int(args[args.index("--mib") + 1]) if "--mib" in args else 1536
    base = json.loads(raw)
    work = tempfile.mkdtemp()
    cwasm = os.path.join(work, "ocr.cwasm")
    subprocess.run(["wasmtime", "compile", wasm, "-o", cwasm], check=True, capture_output=True)
    cmd = ["wasmtime", "run", "--allow-precompiled", "-W", f"max-memory-size={mib * 1024 * 1024}",
           "--dir", f"{data}::/data", cwasm]
    scale = 1 if sys.platform == "darwin" else 1024  # ru_maxrss: bytes on macOS, KiB on Linux
    stats, cursor, md_bytes, docs_seen = [], None, 0, 0
    while True:
        inp = dict(base)
        if cursor:
            inp["cursor"] = cursor
        t0 = time.time()
        p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        p.stdin.write(json.dumps(inp).encode()); p.stdin.close()
        out = p.stdout.read(); err = p.stderr.read()
        _, status, ru = os.wait4(p.pid, 0)
        secs = time.time() - t0
        if status != 0:
            sys.exit(f"call {len(stats) + 1} failed: {err.decode()[:300]}")
        res = json.loads(out)
        res = res.get("response", res)
        docs = res.get("documents", [])
        md_bytes += sum(len(d.get("markdown", "")) for d in docs)
        docs_seen += len(docs)
        stats.append((secs, ru.ru_maxrss * scale / 1048576))
        cursor = (res.get("next") or {}).get("cursor")
        if not cursor or (calls and len(stats) >= calls):
            break
    t = [s for s, _ in stats]
    print(json.dumps({
        "calls": len(stats), "finished": cursor is None,
        "first_call_s": round(t[0], 2), "last_call_s": round(t[-1], 2),
        "total_s": round(sum(t), 1), "slowest_call_s": round(max(t), 2),
        "peak_mib": round(max(m for _, m in stats)),
        "markdown_mib": round(md_bytes / 1048576, 1),
    }))


main()
