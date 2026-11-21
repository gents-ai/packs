# Ember Isles

A three-player island-control game, played by AI players inside Gents and watched on a page in the app.

## Houses and turn order

Ash, then Brine, then Cinder. Each House is played by its own AI agent, and each agent has a distinct character.

## The map

24 isles in four groups.

| Group | Isles |
| --- | --- |
| Northreach | Frost, Gull, Harrow, Ivory, Juniper, Kestrel |
| Eastfold | Lantern, Marrow, Nettle, Ochre, Pike, Quill |
| Southmere | Reed, Salt, Thorn, Umber, Vale, Wren |
| Westhollow | Yarrow, Zephyr, Alder, Birch, Cairn, Dune |

Isle order for dealing: Frost, Gull, Harrow, Ivory, Juniper, Kestrel, Lantern, Marrow, Nettle, Ochre, Pike, Quill, Reed, Salt, Thorn, Umber, Vale, Wren, Yarrow, Zephyr, Alder, Birch, Cairn, Dune.

Isles connected by a strait (36, each works both ways):

- Frost - Gull
- Gull - Harrow
- Harrow - Ivory
- Ivory - Juniper
- Juniper - Kestrel
- Kestrel - Frost
- Frost - Ivory
- Lantern - Marrow
- Marrow - Nettle
- Nettle - Ochre
- Ochre - Pike
- Pike - Quill
- Quill - Lantern
- Lantern - Ochre
- Reed - Salt
- Salt - Thorn
- Thorn - Umber
- Umber - Vale
- Vale - Wren
- Wren - Reed
- Reed - Umber
- Yarrow - Zephyr
- Zephyr - Alder
- Alder - Birch
- Birch - Cairn
- Cairn - Dune
- Dune - Yarrow
- Yarrow - Birch
- Gull - Lantern
- Ivory - Pike
- Quill - Reed
- Nettle - Thorn
- Vale - Zephyr
- Wren - Alder
- Cairn - Frost
- Dune - Kestrel

## Dice

All randomness comes from one integer `x`, starting at the game's `seed`. To draw, set `x = (x * 1103515245 + 12345) mod 2147483648`, then use `x >> 16`. A die is `((x >> 16) mod 6) + 1`. Draws are used in the order they happen in the game, so a game can be replayed from its log.

## Setup

Shuffle the isle order: for `i` from 23 down to 1, draw, let `j = (x >> 16) mod (i + 1)`, and swap positions `i` and `j`. Deal in shuffled order to Ash, Brine, Cinder, repeating. Each isle starts with 3 ships.

## A turn

Skip a House with no isles. The steps happen in this order, and a House cannot go back to an earlier step.

1. **Income.** The House receives `max(2, floor(isles held / 3))` ships, plus 3 for each group it holds entirely.
2. **Deploy.** It places all of them on its own isles, in one or more deployments.
3. **Raid, any number of times.** Choose an own isle A with at least 2 ships, a connected enemy isle B, and a commitment `c` from 1 to `min(3, ships on A - 1)`. Draw the attacker's die, then the defender's die. Attack is `die + c`. Defense is `die + min(3, ships on B)`. Ties go to the defender. If the defender wins, A loses 1 ship. If the attacker wins, B loses 1 ship; if B reaches 0, the attacker captures it and moves `c` ships from A to B.
4. **Regroup, at most once.** Move any number of ships between two connected own isles, leaving at least 1 behind.
5. **End the turn.**

## Winning

A House wins immediately when it holds 18 or more isles, or is the only House with isles. A game document sets `max_rounds` (default 20). After the last round, the winner is the House with the most isles, then the most ships, then the earliest in turn order.

## The log

A game is one `IsleGame` document (`game_id`, `seed`, `max_rounds`) and an append-only log of `IsleEvent` documents. Log events are never changed or deleted. You may add fields, but every event has `game_id`, `seq` (dense from 1), `kind`, `actor` (the House), `isle`, `target`, `ships` and `detail` (JSON text), used like this:

| kind | Meaning | Fields |
| --- | --- | --- |
| `deal` | Setup, always seq 1 | `detail`: `{"isles": {"Frost": "Ash", ...}}` |
| `income` | Ships received | `actor`, `ships` |
| `deploy` | Ships placed | `actor`, `isle`, `ships` |
| `raid` | A raid from `isle` on `target` | `actor`, `isle`, `target`, `ships` (the commitment), `detail`: `{"attack_die": n, "defense_die": n}` |
| `capture` | Immediately after a raid that took the isle | `actor`, `isle`, `target`, `ships` (ships moved) |
| `regroup` | Ships moved between own isles | `actor`, `isle`, `target`, `ships` |
| `end_turn` | Turn finished | `actor` |
| `win` | Game over | `actor` (the winner) |
