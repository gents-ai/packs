# claim_graph_live

Live model run of the claim_graph pack. A scenario pack for `make live`; not published.

The seed is a `ResearchRequest` (`request_id` is the run id, `topic` the prompt, `graph_id` `flipper-fcc`). The pack's `research-request` trigger runs the researcher, which starts the graph, fetches sources through `claim_sources`, files claims through the gate and writes a `ResearchReceipt`. That receipt runs the pack's reviewer and this scenario's one trigger, which asks the expert (`claim-sme`) to answer from the new graph. The run passes when the expert's request completes and the home holds at least one `Source`, five `Claim`s, one `GateReceipt` and one `ResearchReceipt`.

## Configuration

Depends on the `claim_graph` pack. `GENTS_LIVE_ENDPOINT` and `GENTS_LIVE_MODEL` select the OpenAI-compatible model (default ollama at `http://127.0.0.1:11434/v1` with `qwen3.8:latest`). `claim_sources` needs outbound HTTPS, which `make live` grants with `--grant-authority`.

## Usage

```sh
make live LIVE_PACK=claim_graph LIVE_ENDPOINT=http://127.0.0.1:11434/v1 LIVE_MODEL=qwen3.8:latest
```

`max_sources` and `create_graph` stay unset, because the scenario seed writes only string fields: the researcher fetches at most 4 sources and starts the graph because none exists.
