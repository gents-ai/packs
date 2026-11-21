"""Grade a built help desk app on a node. Read-only.

  python3 -I check_helpdesk.py HOME [SINCE] [NEW_COUNT]

Compares the node's tickets with tickets.csv in the run's workspace.
"""
import csv
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import app_checks as A

PRIORITIES = {"urgent", "normal", "low"}


def main():
    home = os.path.expanduser(sys.argv[1])
    since = sys.argv[2] if len(sys.argv) > 2 else None
    new = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    source = list(csv.DictReader(open(os.path.join(os.path.dirname(home), "workspace", "tickets.csv"))))
    key = A.agent_key(home)
    eng = A.engineers(home, key)
    checks, bad = {}, []

    tickets = A.rows(home, "DeskTicket", ["ticket_id", "customer_id", "status", "priority", "summary"]) or []
    customers = A.rows(home, "DeskCustomer", ["customer_id", "name"]) or []
    ids = [t["ticket_id"] for t in tickets]
    checks["every_ticket_imported"] = set(ids) == {r["ticket_id"] for r in source}
    checks["no_duplicates"] = bool(ids) and len(ids) == len(set(ids)) and \
        len(customers) == len({c["customer_id"] for c in customers}) == len({r["customer_id"] for r in source})
    by_id = {t["ticket_id"]: t for t in tickets}
    checks["status_matches_file"] = bool(ids) and all(
        by_id.get(r["ticket_id"], {}).get("status") == r["status"] for r in source)
    checks["every_ticket_triaged"] = bool(ids) and all(
        t.get("priority") in PRIORITIES and 0 < len((t.get("summary") or "").strip()) <= 120 and "\n" not in t["summary"].strip()
        for t in tickets)

    fires = A.event_fires(home, since, A.after)
    requests = A.rows(home, "AgentRequest", [key, "caused_by_trigger_id", "created_at"]) or []
    triggered = [r for r in requests if r.get("caused_by_trigger_id") and A.after(r.get("created_at"), since)]
    checks["triage_runs_on_its_own"] = len(triggered) >= max(1, new)
    checks["each_ticket_triaged_once"] = bool(fires) and len(fires) <= 2 * max(1, new)

    found = json.loads(A.run("gents", "query", "search", "--home", home, "--collection", "DeskTicket",
                             "--text", "gyroscope", "--search-field", "subject", "--search-field", "body",
                             "--field", "ticket_id") or "{}").get("results", [])
    checks["search_finds_a_ticket"] = [r.get("ticket_id") for r in found if r.get("_score", 1) > 0] == ["T-1013"]

    checks["page_installed"], checks["page_lints"] = A.pages(home)
    checks["packaged"] = A.packaged(home)
    checks["agents_have_only_their_tools"] = A.narrow_agents(home, key, eng)

    if len(fires) > 100:
        bad.append(f"{len(fires)} trigger fires")
    working = all(checks.values()) and not bad
    print(json.dumps({"working": working, "checks": checks, "bad": bad, "fires": len(fires),
                      "player_sessions": len({f.get("session_id") for f in fires}),
                      "detail": {"games": [], "tickets": len(tickets), "customers": len(customers)}}, indent=1))


if __name__ == "__main__":
    main()
