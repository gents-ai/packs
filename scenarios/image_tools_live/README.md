# image_tools_live

Live model run of the image_tools pack's Image helper on three small photos. A scenario pack for `make live`; not published.

## Configuration

Depends on the `image_tools` pack. `GENTS_LIVE_ROOT` is the tool root holding a copy of `fixtures/`; `GENTS_LIVE_ENDPOINT` and `GENTS_LIVE_MODEL` select the OpenAI-compatible model.

## Usage

```sh
make live LIVE_PACK=image_tools LIVE_ENDPOINT=http://workstation-1:8000/v1
```
