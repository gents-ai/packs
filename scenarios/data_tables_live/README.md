# data_tables_live

Live model run of the data_tables pack's Data analyst on a small CSV. A scenario pack for `make live`; not published.

## Configuration

Depends on the `data_tables` pack. `GENTS_LIVE_ROOT` is the tool root holding a copy of `fixtures/`; `GENTS_LIVE_ENDPOINT` and `GENTS_LIVE_MODEL` select the OpenAI-compatible model.

## Usage

```sh
make live LIVE_PACK=data_tables LIVE_ENDPOINT=http://workstation-1:8000/v1
```
