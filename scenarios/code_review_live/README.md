# code_review_live

Live model run of the code_review `review-changes` workflow on two small repositories. A scenario pack for `make live`; not published.

## Fixtures

`fixtures/make_repos.sh` builds two git repositories, each with a `base` and a `head` commit, from `fixtures/base`:

- `review_seeded_bug`: the head adds one off-by-one at `src/lib.rs:12`.
- `review_clean_refactor`: the head rewrites `page_count` with `div_ceil` and changes no behaviour.

## What the run asserts

`experiment.json` checks the driver's tool results: the seeded review confirms a finding at `src/lib.rs` line 12, the clean review completes with 0 confirmed findings (the plugin confirms only at confidence 80 or more), and `fixtures/counts_balance.sh` accepts a review's counts, labelled `seeded` or `clean` so each review is checked on its own (candidate = confirmed + refuted + unverified). The `review_changes` needles assume the compact JSON of the code_review result contract, read from the job's tool-call output once it completes.

## Configuration

Depends on `code_review`, which ships the `review_changes` job plugin. `GENTS_LIVE_ROOT` holds a copy of `fixtures/`; `GENTS_LIVE_ENDPOINT` and `GENTS_LIVE_MODEL` select the model. `pack_config.json` grants the `gents/review_changes` entry a `job` block that targets the four code-review-* agents.

## Usage

```sh
make live LIVE_PACK=code_review LIVE_ENDPOINT=http://workstation-1:8000/v1
```

A live run is not part of the pack gate. Run each check three times per backend and compare the confirm rates.
