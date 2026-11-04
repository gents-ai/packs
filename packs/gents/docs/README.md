# Docs pack

The Docs pack provides a headless starting point for document review. It
registers document revision and suggestion collections, a Document editor
agent, and narrow datastore tools. The agent reads one named revision and
saves a proposed complete Markdown replacement for a person to review. It
does not change the revision itself.

## Install and bind a model

Install into an initialized Gents home and bind the slot to an existing
inference profile:

```sh
gents pack install ./packs/gents/docs --home <home> --inference-slot docs_editor=<profile_id>
```

The pack does not configure a provider or create an inference profile. Bind the
slot to any compatible profile the operator has configured. Document content
used in an agent request is sent to that profile's provider, so choose an
appropriate provider and its access policy for the document. The `gents/ocr`
pack can extract files such as PDFs and Office documents into Markdown; install
and use it separately when an import is needed. The Docs pack does not
implicitly attach OCR or grant access to files.

## Data and tool contract

`DocsRevision` contains `document_id`, `revision_id`, optional
`parent_revision_id`, `title`, and Markdown `content`. `DocsSuggestion`
contains `document_id`, `base_revision_id`, `suggestion_id`, replacement
Markdown `content`, and `summary`. IDs are supplied by the caller, who should
use fresh, collision-resistant IDs. The format has no enforced uniqueness,
immutable history, verified ancestry, or compare-and-swap. Parent and base IDs
are references for the caller, not proof that a revision is current or that a
suggestion applies cleanly. A client must check for missing, duplicate and
concurrent revisions and ask a person to resolve conflicts before saving.

The `docs-editor` datastore surface declares `list_docs_revisions`,
`create_docs_revision`, and `list_docs_suggestions`. The separate
`docs-reviewer` surface declares `read_docs_revision`, requiring an exact
document and revision ID, and `create_docs_suggestion`. Only `docs-reviewer`
is granted to the Document editor agent. The agent can suggest a replacement,
but it cannot create a document revision through its declared Docs tools.

A node operator can create a revision with the headless CLI. For example,
generate fresh IDs in real use:

```sh
gents document create DocsRevision --home <home> --json '{"document_id":"document-123","revision_id":"revision-123","title":"Agreement","content":"# Agreement\n\nDraft terms."}'
```

With the same Gents home and the `docs-editor` agent selected, ask the agent
to read the supplied document ID and revision ID and suggest a change. For
example, use `gents chat --home <home> --agent-id docs-editor` and ask it to review
`document-123` at `revision-123`, providing a fresh suggestion ID. The agent
should read the revision before creating a suggestion. A client may read the
suggestion through the same authorized datastore API:

```graphql
query {
  DocsSuggestion(filter: {document_id: {_eq: "document-123"}}) {
    _docID document_id base_revision_id suggestion_id content summary
  }
}
```

Bounded tool reads can truncate long strings. When `read_docs_revision` reports
`field_recovery`, recover the field with `field_page` starting at byte offset 0;
continue with the returned `next_offset_bytes` and `value_hash` as
`expected_hash` until complete. Keep the document and revision filters on each
call, and restart if the value changed. A bounded row query may also return a
`limit_note`: narrow the filter or raise its limit where supported. Never create
a replacement from partial content or assume a capped result is complete.

To accept a proposal, a person or authorized client must review it against the
current document, resolve any conflict, and create a new `DocsRevision` with a
new revision ID and the reviewed content. The pack does not implement an
accept operation, live editor, typing indicators, agent-memory sharing or
multiplayer merging.

## Access and provenance

The pack does not install access-control policies on its collections. Configure
the node, authenticated clients and DefraDB document ACP policies before
sharing any documents. The pack's field and tool declarations are not
access-control policies: a document ID or an agent prompt does not establish
who may read or write. Apply policy separately to the revision and suggestion
collections, and check permissions on every client and runtime boundary.

A shared document does not automatically grant access to the creator's agent,
conversation, files or model context. Sharing those requires explicit consent
and authorization at their existing owners. The pack does not establish trusted
human attribution or cryptographically verified document history; caller-supplied
IDs and parent links must not be presented as such. Do not treat a suggestion
as approved or as legal, financial, or other professional advice.

## Completion and failure

A successful `create_docs_suggestion` creates a proposal; it is not acceptance
of a document change. Missing or ambiguous revisions must be reported instead
of guessed. Errors and denied reads do not authorize a fallback to another
revision. Separate proposals and forks may coexist and need review.

## Validation

```sh
gents pack check ./packs/gents/docs
GENTS=<path-to-gents> make test-docs
```

The pack tests cover its configuration, tool boundaries, installation, and
headless creation and reads of revision and suggestion records. They do not
demonstrate a multi-client workflow, policy setup, or a model-driven edit.
