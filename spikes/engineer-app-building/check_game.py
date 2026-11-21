"""Grade a built Ember Isles game on a node. Read-only.

  python3 -I check_game.py HOME

Prints JSON: each check, and whether the build works. Replays the log with its
own copy of the rules, so a model can't pass by writing its own win event.
"""
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

MAP = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "spec", "map.json")))
HOUSES = ["Ash", "Brine", "Cinder"]
ADJACENT = {name: set() for name in MAP["order"]}
for a, b in MAP["edges"]:
    ADJACENT[a].add(b)
    ADJACENT[b].add(a)


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


class Dice:
    def __init__(self, seed):
        self.x = int(seed)

    def draw(self):
        self.x = (self.x * 1103515245 + 12345) % 2147483648
        return self.x >> 16


def replay(game, events):
    """Return the winner, or raise ValueError naming the first illegal event."""
    events = sorted(events, key=lambda e: e["seq"])
    if [e["seq"] for e in events] != list(range(1, len(events) + 1)):
        raise ValueError("seq is not dense from 1")
    dice = Dice(game["seed"])
    order = list(MAP["order"])
    for i in range(23, 0, -1):
        j = dice.draw() % (i + 1)
        order[i], order[j] = order[j], order[i]
    owner = {isle: HOUSES[p % 3] for p, isle in enumerate(order)}
    ships = {isle: 3 for isle in order}
    if not events or events[0]["kind"] != "deal":
        raise ValueError("first event is not deal")
    if json.loads(events[0]["detail"])["isles"] != owner:
        raise ValueError("deal does not match the seed")

    def held(house):
        return [i for i in owner if owner[i] == house]

    def alive():
        return [h for h in HOUSES if held(h)]

    def instant():
        for house in HOUSES:
            if len(held(house)) >= 18 or alive() == [house]:
                return house
        return None

    def final():
        return max(enumerate(HOUSES),
                   key=lambda p: (len(held(p[1])), sum(ships[i] for i in held(p[1])), -p[0]))[1]

    def income(house):
        bonus = sum(3 for names in MAP["groups"].values() if all(owner[n] == house for n in names))
        return max(2, len(held(house)) // 3) + bonus

    round_number, index, position = 1, 0, 1
    max_rounds = int(game.get("max_rounds") or 20)
    while True:
        house = HOUSES[index]
        if held(house):
            def need(kind):
                nonlocal position
                if position >= len(events):
                    raise ValueError(f"log ends before {kind}")
                event = events[position]
                if event["kind"] != kind or event.get("actor") != house:
                    raise ValueError(f"seq {event['seq']}: expected {kind} by {house}, got {event['kind']} by {event.get('actor')}")
                position += 1
                return event
            owed = income(house)
            got = need("income")
            if got["ships"] != owed:
                raise ValueError(f"seq {got['seq']}: income {got['ships']}, expected {owed}")
            while owed:
                placed = need("deploy")
                if owner.get(placed["isle"]) != house or not 1 <= placed["ships"] <= owed:
                    raise ValueError(f"seq {placed['seq']}: illegal deploy")
                ships[placed["isle"]] += placed["ships"]
                owed -= placed["ships"]
            regrouped = False
            while True:
                if position >= len(events):
                    raise ValueError("log ends mid-turn")
                event = events[position]
                kind = event["kind"]
                if event.get("actor") != house:
                    raise ValueError(f"seq {event['seq']}: wrong actor")
                position += 1
                if kind == "end_turn":
                    break
                if kind == "regroup":
                    a, b, n = event["isle"], event["target"], event["ships"]
                    if regrouped or owner.get(a) != house or owner.get(b) != house or b not in ADJACENT[a] or not 1 <= n < ships[a]:
                        raise ValueError(f"seq {event['seq']}: illegal regroup")
                    ships[a] -= n
                    ships[b] += n
                    regrouped = True
                    continue
                if kind != "raid" or regrouped:
                    raise ValueError(f"seq {event['seq']}: unexpected {kind}")
                a, b, c = event["isle"], event["target"], event["ships"]
                if owner.get(a) != house or owner.get(b) in (None, house) or b not in ADJACENT[a] or not 1 <= c <= min(3, ships[a] - 1):
                    raise ValueError(f"seq {event['seq']}: illegal raid")
                detail = json.loads(event["detail"])
                attack, defense = dice.draw() % 6 + 1, dice.draw() % 6 + 1
                if (detail["attack_die"], detail["defense_die"]) != (attack, defense):
                    raise ValueError(f"seq {event['seq']}: dice do not match the seed")
                if attack + c > defense + min(3, ships[b]):
                    ships[b] -= 1
                    if ships[b] == 0:
                        taken = need("capture")
                        if (taken["isle"], taken["target"], taken["ships"]) != (a, b, c):
                            raise ValueError(f"seq {taken['seq']}: wrong capture")
                        ships[a] -= c
                        ships[b] = c
                        owner[b] = house
                        winner = instant()
                        if winner:
                            return finish(events, position, winner)
                else:
                    ships[a] -= 1
        index += 1
        if index == 3:
            index = 0
            if round_number == max_rounds:
                return finish(events, position, final())
            round_number += 1


def finish(events, position, winner):
    if position != len(events) - 1:
        raise ValueError(f"expected the win at seq {position + 1}, log has {len(events)} events")
    if events[position]["kind"] != "win" or events[position].get("actor") != winner:
        raise ValueError(f"win event names {events[position].get('actor')}, rules say {winner}")
    return winner


def after(stamp, since):
    import datetime
    if not since:
        return True
    if not stamp:
        return False
    parse = lambda t: datetime.datetime.fromisoformat(str(t).replace("Z", "+00:00"))
    return parse(stamp) >= parse(since)


def main():
    home = os.path.expanduser(sys.argv[1])
    since = sys.argv[2] if len(sys.argv) > 2 else None
    known = set(json.load(open(sys.argv[3]))) if len(sys.argv) > 3 else set()
    # The agent key is agent_id before the identity change and agent_did after it.
    fields = json.loads(run("gents", "query", "fields", "--home", home, "--collection", "Agent") or '{"fields":[]}')["fields"]
    AF = "agent_did" if any(f["name"] == "agent_did" for f in fields) else "agent_id"
    ENG = {a[AF] for a in (rows(home, "Agent", [AF, "tags"]) or [])
           if "gents:engineer" in (a.get("tags") or []) or str(a[AF]).endswith(":default")}
    checks, bad = {}, []
    games = rows(home, "IsleGame", ["game_id", "seed", "max_rounds"]) or []
    fields = ["game_id", "seq", "kind", "actor", "isle", "target", "ships", "detail"]
    events = rows(home, "IsleEvent", fields) or []
    checks["collections"] = bool(games)

    import app_checks
    checks["packaged"] = app_checks.packaged(home)
    checks["page_installed"], checks["page_lints"] = app_checks.pages(home)
    plugins = json.loads(run("gents", "plugin", "list", "--home", home) or '{"plugins":[]}').get("plugins", [])
    checks["rules_plugin_installed"] = bool(plugins)

    results = []
    for game in games:
        if game["game_id"] in known:
            continue
        log = [e for e in events if e["game_id"] == game["game_id"]]
        if not log:
            continue
        try:
            results.append((game["game_id"], True, replay(game, log), len(log)))
        except (ValueError, KeyError, TypeError) as error:
            results.append((game["game_id"], False, f"{type(error).__name__}: {error}", len(log)))
    legal = [r for r in results if r[1]]
    checks["game_replays_to_a_winner"] = bool(legal)
    detail = {"games": results}

    requests = rows(home, "AgentRequest", [AF, "caused_by_trigger_id", "lifecycle_state"]) or []
    players = [r for r in requests if r[AF] not in ENG]
    triggered = [r for r in players if r.get("caused_by_trigger_id")]
    checks["players_ran_from_triggers"] = len(triggered) >= 3
    # Only event-driven fires count as waking a player. A schedule (a watchdog, a refresh) is not a turn.
    event_triggers = {t["trigger_id"] for t in (rows(home, "Trigger", ["trigger_id", "source"]) or [])
                      if (t.get("source") or {}).get("kind") == "event"}
    fires = [f for f in (rows(home, "TriggerFire", ["trigger_id", "session_id", "created_at"]) or [])
             if after(f.get("created_at"), since) and f["trigger_id"] in event_triggers]
    sessions = sorted({f.get("session_id") for f in fires if f.get("session_id")})
    # Count turns in the graded game even when it is illegal, so a rules bug is not also reported as a wake-up problem.
    graded = (legal or results)[0][0] if results else None
    turns = sum(1 for e in events if e["game_id"] == graded and e["kind"] == "end_turn")
    checks["only_the_mover_is_woken"] = bool(fires) and bool(turns) and len(fires) <= 2 * turns
    # Memory within a game: no trigger uses more sessions than one per house per game (one trigger may serve all three houses).
    per_trigger = {}
    for f in fires:
        per_trigger.setdefault(f["trigger_id"], set()).add(f.get("session_id"))
    checks["one_session_per_house"] = bool(per_trigger) and all(len(v) <= 3 * max(1, len(results)) for v in per_trigger.values())
    tools = rows(home, "Tools", ["tools_id", "host", "self_config"]) or []
    agents = rows(home, "Agent", [AF, "context_id"]) or []
    contexts = {c["context_id"]: c.get("tools_id") for c in (rows(home, "AgentContext", ["context_id", "tools_id"]) or [])}
    by_id = {t["tools_id"]: t for t in tools}
    wide = []
    for agent in agents:
        if agent[AF] in ENG:
            continue
        selected = by_id.get(contexts.get(agent["context_id"]), {})
        if selected.get("self_config") or (selected.get("host") or {}).get("bash"):
            wide.append(agent[AF])
    checks["players_have_only_game_tools"] = bool(agents) and not wide

    if len(fires) > 200:
        bad.append(f"{len(fires)} trigger fires")
    if len(sessions) > 30:
        bad.append(f"{len(sessions)} player sessions")
    working = all(checks.values()) and not bad
    print(json.dumps({"working": working, "checks": checks, "bad": bad,
                      "fires": len(fires), "player_sessions": len(sessions), "detail": detail}, indent=1))


if __name__ == "__main__":
    main()
