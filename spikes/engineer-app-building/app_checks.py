"""Checks shared by the task graders. Read-only against the node."""
import json
import os
import subprocess

SKILLS_PACK = "gents/pack_builder"


def run(*argv):
    return subprocess.run(argv, capture_output=True, text=True).stdout


def rows(home, collection, fields, order=None):
    found, offset = [], 0
    while True:
        argv = ["gents", "query", "find", "--home", home, "--collection", collection,
                "--limit", "1000", "--offset", str(offset)]
        for field in fields:
            argv += ["--field", field]
        if order:
            argv += ["--order", json.dumps([{order: "ASC"}])]
        try:
            page = json.loads(run(*argv))["results"]
        except (ValueError, KeyError):
            return None
        found += page
        if len(page) < 1000:
            return found
        offset += 1000


def agent_key(home):
    fields = json.loads(run("gents", "query", "fields", "--home", home, "--collection", "Agent") or '{"fields":[]}')["fields"]
    return "agent_did" if any(f["name"] == "agent_did" for f in fields) else "agent_id"


def engineers(home, key):
    return {a[key] for a in (rows(home, "Agent", [key, "tags"]) or [])
            if "gents:engineer" in (a.get("tags") or []) or str(a[key]).endswith(":default")}


def pages(home):
    """(a page exists, every page passes the lint). Pages are documents or pack files."""
    import tempfile
    listed = json.loads(run("gents", "ux", "list", "--home", home) or '{"plugins":[]}').get("plugins", [])
    files = [p["file"] for p in listed if p.get("file")]
    docs = [d for d in (rows(home, "Page", ["page_id", "module", "css", "enabled"]) or []) if d.get("enabled") is not False]
    with tempfile.TemporaryDirectory() as tmp:
        for i, doc in enumerate(docs):
            folder = os.path.join(tmp, str(i))
            os.mkdir(folder)
            open(os.path.join(folder, "plugin.js"), "w").write(doc.get("module") or "")
            if doc.get("css"):
                open(os.path.join(folder, "plugin.css"), "w").write(doc["css"])
            files.append(os.path.join(folder, "plugin.js"))
        ok = bool(files) and all(subprocess.run(["gents", "ux", "lint", f], capture_output=True).returncode == 0 for f in files)
    return bool(files), ok


def packaged(home):
    """The app is a pack: one is installed, or a pack in the workspace passes the install checks."""
    packs = json.loads(run("gents", "pack", "list", "--home", home) or '{"packs":[]}').get("packs", [])
    if any(p.get("installed") and p.get("pack") != SKILLS_PACK for p in packs):
        return True
    workspace = os.path.join(os.path.dirname(home), "workspace")
    for base, dirs, files in os.walk(workspace):
        if base[len(workspace):].count(os.sep) > 3:
            dirs[:] = []
            continue
        if "manifest.json" in files:
            if subprocess.run(["gents", "pack", "check", base], capture_output=True).returncode == 0:
                return True
    return False


def narrow_agents(home, key, skip):
    """Agents other than the Engineer hold neither self-config nor a shell."""
    tools = {t["tools_id"]: t for t in (rows(home, "Tools", ["tools_id", "host", "self_config"]) or [])}
    contexts = {c["context_id"]: c.get("tools_id") for c in (rows(home, "AgentContext", ["context_id", "tools_id"]) or [])}
    agents = rows(home, "Agent", [key, "context_id"]) or []
    wide = []
    for agent in agents:
        if agent[key] in skip:
            continue
        selected = tools.get(contexts.get(agent["context_id"]), {})
        if selected.get("self_config") or (selected.get("host") or {}).get("bash"):
            wide.append(agent[key])
    return bool(agents) and not wide


def event_fires(home, since, after):
    triggers = {t["trigger_id"] for t in (rows(home, "Trigger", ["trigger_id", "source"]) or [])
                if (t.get("source") or {}).get("kind") == "event"}
    return [f for f in (rows(home, "TriggerFire", ["trigger_id", "session_id", "created_at"]) or [])
            if after(f.get("created_at"), since) and f["trigger_id"] in triggers]


def after(stamp, since):
    import datetime
    if not since:
        return True
    if not stamp:
        return False
    parse = lambda t: datetime.datetime.fromisoformat(str(t).replace("Z", "+00:00"))
    return parse(stamp) >= parse(since)
