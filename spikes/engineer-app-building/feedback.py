"""Turn a grade into the next message for the builder.
  feedback.py GRADE.json OUT.txt
Failures go back as what a user would notice, never as the design we're looking for."""
import json
import sys

SAY = {
    "collections": "I can't find any game data on the node.",
    "app_pack_installed": "The game isn't installed on this node.",
    "page_installed": "There's nothing in the app where I can watch the game.",
    "page_lints": "The game's page fails the page lint.",
    "rules_plugin_installed": "Nothing but the AI players themselves is enforcing the rules, so I can't trust the result.",
    "game_replays_to_a_winner": "No game has finished with a correct winner.",
    "players_ran_from_triggers": "The AI players aren't taking their turns on their own.",
    "only_the_mover_is_woken": "Players are being woken when it isn't their turn, which wastes a lot of inference.",
    "one_session_per_house": "The players keep starting new sessions, so they don't remember the game.",
    "players_have_only_game_tools": "The players have tools they don't need for the game.",
    "packaged": "The app isn't packaged, so I can't install it on my other node.",
    "every_ticket_imported": "The tickets on the node don't match the rows in tickets.csv.",
    "no_duplicates": "There are duplicate tickets or customers.",
    "status_matches_file": "Some tickets have a different status from the file.",
    "every_ticket_triaged": "Some tickets have no valid priority or one-line summary.",
    "triage_runs_on_its_own": "New tickets aren't being triaged on their own.",
    "each_ticket_triaged_once": "Tickets are being triaged more than once, which wastes inference.",
    "search_finds_a_ticket": "Searching tickets for a word in a ticket's subject doesn't find just that ticket.",
    "agents_have_only_their_tools": "The app's agents have tools they don't need.",
}

grade = json.load(open(sys.argv[1]))
failing = [SAY[k] for k, v in grade["checks"].items() if not v] + [f"It created {b}." for b in grade["bad"]]
errors = [g[2] for g in grade["detail"]["games"] if not g[1]][-1:]
lines = ["It isn't working yet:", *[f"- {f}" for f in failing]]
if errors:
    lines.append(f"I replayed the latest game's log against the rules in SPEC.md and it failed: {errors[0]}")
lines.append(sys.argv[3] if len(sys.argv) > 3 else "Fix these, keep what already works, and start a new game with max_rounds 1 to confirm it reaches a winner.")
open(sys.argv[2], "w").write("\n".join(lines) + "\n")
