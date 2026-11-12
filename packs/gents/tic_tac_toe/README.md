# Tic-tac-toe pack

A reference board for two people on separate Gents nodes. A game and its moves
are branchable documents. The page folds the moves into a board and refuses
ambiguous, invalid or concurrent histories. A person can ask the agent in the
selected session to make a move. The agent has its own model slot.

```sh
gents config pack install ./packs/gents/tic_tac_toe --home <home> --inference-slot game_player=<profile-id>
```

Enable the Tic-tac-toe page in the desktop after installing the pack. The same
pack schema must be installed on both nodes. A runtime with support for binding
pack page calls to the selected node is required; older hosts show an update
message. Use the sharing flow to grant access to the pack and, separately,
the intended agent. The page sends an agent request only after a user clicks
its button; the selected session must be authorized for that agent.

Each `TicTacToeMove` names a game, a fresh move ID, the preceding move ID (empty
for the first move), mark and cell index. Cells 0–8 run left to right and top to
bottom. X goes first. Wins, draws, occupied cells and alternating turns are
checked when rendering and before writing. A pending write keeps its identity
and must be checked before another write, because a lost receipt is not proof
of non-delivery.

A playable history is read in one bounded query. More than nine moves or an
incomplete result stops play, since separate pages do not form a snapshot.

The current generic data surfaces do not enforce game rules, bind a player to
a verified DID, or provide atomic compare-and-set. A peer can bypass page-side
checks, and two concurrent legal moves can still form competing children. The
page reports these cases and stops rather than choosing a winner. The pack does
not install document access policies. Configure and verify ACP and pairing for
both collections before sharing data. It does not grant access to an agent,
private conversation, model account or other packs. Agent access requires a
separate, explicit grant. The page uses scoped query observations when the host
provides them and falls back to client events and periodic refresh otherwise.
Cross-node delivery is not guaranteed by the pack itself.

## Checks

```sh
gents config pack check packs/gents/tic_tac_toe
GENTS=<path-to-gents> make test-tic_tac_toe
node --test packs/gents/tic_tac_toe/tests/game_test.mjs
GENTS_DESKTOP_ROOT=<gents-checkout> node packs/gents/tic_tac_toe/tests/browser_check.mjs
```

The unit cases exercise game folding, the pack cases cover installation and
declarations, and the browser fixture exercises the page with a test data
provider. They do not demonstrate multi-node authorization, verified player
attribution, notifications, agent admission, or a complete in-app invitation.
