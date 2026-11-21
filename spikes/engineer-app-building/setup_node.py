"""Set a fresh node's Engineer prompt, skills and shell without a model turn.
  setup_node.py EXPORT_DIR [--prompt FILE] [--skills-pack DIR] [--no-shell] [--plugin NS/NAME]...
Edits an exported manifest root in place; the caller runs `gents config apply`."""
import argparse, json, os
p = argparse.ArgumentParser()
p.add_argument("root"); p.add_argument("--prompt"); p.add_argument("--skills-pack"); p.add_argument("--no-shell", action="store_true"); p.add_argument("--plugin", action="append", default=[])
a = p.parse_args()
path = os.path.join(a.root, "pack_config.json")
c = json.load(open(path))
eng = next((a for a in c["agents"] if "gents:engineer" in (a.get("tags") or [])), None)
ctx = next(x for x in c["contexts"] if (eng and x["context_id"] == eng["context_id"]) or x["context_id"].endswith("default-context"))
if a.prompt:
    open(os.path.join(a.root, ctx["system_prompt"]), "w").write(open(a.prompt).read())
if a.skills_pack:
    ctx["skill_ids"] = [s["skill_id"] for s in json.load(open(os.path.join(a.skills_pack, "pack_config.json")))["skills"]]
if a.no_shell:
    tools = next(t for t in c["tools"] if t["tools_id"] == ctx.get("tools_id") or t["tools_id"].endswith("default-tools"))
    tools["host"]["bash"]["mode"] = "Off"
if a.plugin:
    tools = next(t for t in c["tools"] if t["tools_id"] == ctx.get("tools_id") or t["tools_id"].endswith("default-tools"))
    integ = tools.get("integrations") or {}
    integ["plugins"] = (integ.get("plugins") or []) + [{"plugin": name, "tool_calls": True} for name in a.plugin]
    tools["integrations"] = integ
json.dump(c, open(path, "w"), indent=2)
print("skills", ctx.get("skill_ids"), "no_shell", a.no_shell, "prompt", bool(a.prompt))
