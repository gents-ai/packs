# Mailbox surface asset

## Installation

```bash
gents pack install ./packs/gents/mailbox --home <home>
gents pack install gents/mailbox --home <home>   # once published to the registry
```

This pack declares no inference slots.

`gents pack install mailbox --home <home>` materializes this reusable surface
asset under the home's pack assets; it is not a complete desired-state root and
does not write runtime configuration. Incorporate the surface into a document
pack's canonical `pack_config.json`, bind its `node_did` to the consuming
pack's owning node, then reference `mailbox-writes` from the
intended context's `Tools.datastore` configuration after reviewing its declared
fields. There is no graph or seed in this asset pack.

## Bindings and prerequisites

None: this is an assets pack with no inference slots and no runtime
configuration of its own.

## Authority

The surface grants the stamped `file_mailbox_item` tool. Callers supply
`title` and optional `summary` and `payload`; the runtime stamps ownership,
provenance, and routing. The example uses the canonical event notification
policy: a `flag` requiring `ack`, with identity derived from the current event.
Packs copy or reference it and explicitly attach `mailbox-writes` only to
contexts whose agents may ask their human owner for attention. It is not
granted by default.

## Inputs and outputs

Input: none from this pack directly; a consuming pack's agent calls
`file_mailbox_item`. Output: one `MailboxItem` document per call.

## Completion and failure

Not applicable: this pack installs a reusable surface definition and does
not start requests.

## Validation

```bash
gents pack check ./packs/gents/mailbox
gents pack test ./packs/gents/mailbox
make test-mailbox
```

`tests/install.json` pins the files an install materializes and asserts a
remove releases them.

## Operational history

None recorded yet.
