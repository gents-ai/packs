# charts_live

Live model run of the charts pack's Chart maker on a small CSV. A scenario pack for `make live`; not published.

## Configuration

Depends on the `charts` pack. `GENTS_LIVE_ROOT` is the tool root holding a copy of `fixtures/`; `GENTS_LIVE_ENDPOINT` and `GENTS_LIVE_MODEL` select the OpenAI-compatible model.

## Usage

```sh
make live LIVE_PACK=charts LIVE_ENDPOINT=http://workstation-1:8000/v1
```
