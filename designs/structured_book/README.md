# structured_book and browser_download: design package

Design for porting Shelf's book pipeline (PDF or scans → OCR → table of contents → chapters → paragraphs → canonical text with quote validation) into Gents packs, built on `gents/ocr`, plus a `browser_download` pack that fetches open-access and public-domain sources and arbitrary URLs.

This is a design snapshot for comparison and review. No pack code yet.

| File | What it is |
|---|---|
| `DESIGN.md` | The design: both packs' manifests and pack_config outlines, schemas, behaviors (each mapped to its Shelf prompt), plugins, trigger wiring, end-to-end scenario, the implementation work-list, operator decisions, and a revision log answering every review item |
| `shelf-inventory.md` | Stage-by-stage inventory of Shelf's pipeline: agents, prompts, tools, schemas, validation, and the portable work units |
| `gents-pack-guide.md` | How packs work as of gents v0.20 and main, from source: manifest and pack_config fields, afterburner plugins and their sandbox, triggers vs graphs, pack dependencies, slot binding |
| `download-prior-art.md` | Prior art and the minimal tool set for the download pack |
| `reviews/` | Three adversarial reviews (fidelity to Shelf, gents feasibility, work-list), with round-by-round re-reviews |

Paths written as `$SRC/github.com/...` refer to a local checkout root.

Status: three revision rounds. Blocking review items went 24 → 14 → 7 → 6; the remaining six are small (see the round 3 sections in `reviews/`). Operator decisions are listed at the end of `DESIGN.md`.
