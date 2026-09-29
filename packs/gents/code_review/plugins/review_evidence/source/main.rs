//! review_evidence plugin: the code_review pack's prepare step. It turns
//! the host's collected `git_diff` and `read_only_workspace` facts into the
//! entry's immutable paged evidence documents and its actual job input,
//! ported verbatim from gents `crates/gents/src/graph_package/entry.rs`
//! (`code_review_evidence`, `split_evidence_packet`,
//! `evidence_page_inputs`, `prepare_code_review_run`), which stays there
//! only as the byte-identity equivalence proof's reference implementation.
//!
//! One JSON envelope on stdin, `{"input": <admitted entry input>, "nonce":
//! <per-run id>, "host": {"git_diff": {...}, "workspace": {...}}}`; one
//! JSON result on stdout, `{"input": <entry input>, "documents": [...]}`.
//! Diagnostics go to stderr.
use std::io::Read;

use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The largest byte span one evidence chunk carries; must stay well under a
/// page's document-size ceiling.
const EVIDENCE_CHUNK_MAX_BYTES: usize = 1_800;
/// Chunks per `CodeReviewEvidencePage` document.
const EVIDENCE_CHUNKS_PER_PAGE: usize = 16;

#[derive(Debug, Serialize)]
struct Document {
    collection: String,
    fields: Value,
}

#[derive(Debug, Serialize)]
struct PluginOutput {
    input: Value,
    documents: Vec<Document>,
}

fn main() {
    let mut stdin = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("review_evidence: reading stdin: {err}");
        std::process::exit(1);
    }
    match run(&stdin) {
        Ok(output) => match serde_json::to_string(&output) {
            Ok(text) => println!("{text}"),
            Err(err) => {
                eprintln!("review_evidence: serializing the output: {err}");
                std::process::exit(1);
            }
        },
        Err(err) => {
            eprintln!("review_evidence: {err}");
            std::process::exit(1);
        }
    }
}

/// Splits `packet` into chunks of at most [`EVIDENCE_CHUNK_MAX_BYTES`]
/// bytes, backing off from a multibyte character rather than splitting it.
fn split_evidence_packet(packet: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < packet.len() {
        let mut end = (start + EVIDENCE_CHUNK_MAX_BYTES).min(packet.len());
        while !packet.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(packet[start..end].to_owned());
        start = end;
    }
    chunks
}

/// Builds one `CodeReviewEvidencePage` document per
/// [`EVIDENCE_CHUNKS_PER_PAGE`] chunks, padding the final page's unused
/// chunk slots with empty strings.
fn evidence_page_inputs(
    evidence_id: &str,
    evidence_sha256: &str,
    evidence_byte_count: usize,
    chunks: &[String],
) -> Vec<Value> {
    let page_count = chunks.len().div_ceil(EVIDENCE_CHUNKS_PER_PAGE);
    let mut pages = Vec::with_capacity(page_count);
    for page in 0..page_count {
        let first = page * EVIDENCE_CHUNKS_PER_PAGE;
        let mut fields = Map::new();
        fields.insert(
            "page_key".to_owned(),
            Value::String(format!("{evidence_id}:{page:08}")),
        );
        fields.insert(
            "evidence_id".to_owned(),
            Value::String(evidence_id.to_owned()),
        );
        fields.insert("page_index".to_owned(), Value::String(page.to_string()));
        fields.insert(
            "page_count".to_owned(),
            Value::String(page_count.to_string()),
        );
        fields.insert(
            "evidence_chunk_count".to_owned(),
            Value::String(chunks.len().to_string()),
        );
        fields.insert(
            "evidence_byte_count".to_owned(),
            Value::String(evidence_byte_count.to_string()),
        );
        fields.insert(
            "evidence_sha256".to_owned(),
            Value::String(evidence_sha256.to_owned()),
        );
        for slot in 0..EVIDENCE_CHUNKS_PER_PAGE {
            fields.insert(
                format!("evidence_chunk_{slot}"),
                Value::String(chunks.get(first + slot).cloned().unwrap_or_default()),
            );
        }
        pages.push(Value::Object(fields));
    }
    pages
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    path: &str,
    field: &str,
) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path}.{field} must be a string"))
}

fn run(stdin: &str) -> Result<PluginOutput, String> {
    let envelope: Value =
        serde_json::from_str(stdin).map_err(|err| format!("invalid JSON input: {err}"))?;
    let envelope = envelope
        .as_object()
        .ok_or_else(|| "input must be a JSON object".to_string())?;
    let input = envelope
        .get("input")
        .and_then(Value::as_object)
        .ok_or_else(|| "\"input\" must be a JSON object".to_string())?;
    let host = envelope
        .get("host")
        .and_then(Value::as_object)
        .ok_or_else(|| "\"host\" must be a JSON object".to_string())?;
    let nonce = envelope
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or_else(|| "\"nonce\" must be a string".to_string())?;
    let git_diff = host
        .get("git_diff")
        .and_then(Value::as_object)
        .ok_or_else(|| "\"host.git_diff\" must be a JSON object".to_string())?;
    let workspace = host
        .get("workspace")
        .and_then(Value::as_object)
        .ok_or_else(|| "\"host.workspace\" must be a JSON object".to_string())?;

    let base_sha = required_str(git_diff, "host.git_diff", "base_sha")?;
    let head_sha = required_str(git_diff, "host.git_diff", "head_sha")?;
    let name_status = required_str(git_diff, "host.git_diff", "name_status")?;
    let stat = required_str(git_diff, "host.git_diff", "stat")?;
    let patch = required_str(git_diff, "host.git_diff", "patch")?;
    let workspace_id = required_str(workspace, "host.workspace", "workspace_id")?;
    let workspace_owner = required_str(workspace, "host.workspace", "owner_agent_did")?;
    let workspace_authority = required_str(workspace, "host.workspace", "authority")?;
    let focus = required_str(input, "input", "focus")?;

    let summary = format!(
        "PINNED BASE: {base_sha}\nPINNED HEAD: {head_sha}\n\nCHANGED FILES:\n{name_status}\n\nDIFF STAT:\n{stat}"
    );
    let packet = format!("{summary}\n\nCOMPLETE PATCH:\n{patch}");
    let chunks = split_evidence_packet(&packet);
    let byte_count = packet.len();
    let sha256 = format!("{:x}", Sha256::digest(packet.as_bytes()));

    let manifest = json!({
        "evidence_id": nonce,
        "format_version": "1",
        "page_count": chunks.len().div_ceil(EVIDENCE_CHUNKS_PER_PAGE).to_string(),
        "evidence_chunk_count": chunks.len().to_string(),
        "evidence_byte_count": byte_count.to_string(),
        "evidence_sha256": sha256,
    });
    let pages = evidence_page_inputs(nonce, &sha256, byte_count, &chunks);

    let mut documents = vec![Document {
        collection: "CodeReviewEvidenceManifest".to_owned(),
        fields: manifest,
    }];
    documents.extend(pages.into_iter().map(|fields| Document {
        collection: "CodeReviewEvidencePage".to_owned(),
        fields,
    }));

    let entry_input = json!({
        "repository_path": ".",
        "base_ref": base_sha,
        "head_ref": head_sha,
        "workspace_id": workspace_id,
        "workspace_authority": workspace_authority,
        "workspace_owner_agent_did": workspace_owner,
        "lens_count": "4",
        "lens_min": "4",
        "lens_max": "4",
        "pr_number": "",
        "evidence_id": nonce,
        "evidence_summary": summary,
        "evidence_chunk_count": chunks.len().to_string(),
        "focus": focus,
    });

    Ok(PluginOutput {
        input: entry_input,
        documents,
    })
}

#[cfg(test)]
mod tests {
    //! Ported from gents `graph_package/entry/tests.rs`
    //! (`evidence_pages_are_complete_and_bounded`,
    //! `empty_evidence_packet_has_no_rows`): the byte-identity goldens under
    //! `tests/*.json` are this plugin's cross-implementation proof, checked
    //! by `gents pack test`; these are the paging invariants a golden case
    //! cannot itself assert (no fixed patch-size ceiling, no chunk split
    //! across a multibyte boundary, an empty packet pages to nothing).
    use super::*;

    #[test]
    fn evidence_pages_are_complete_and_bounded() {
        let packet = format!("{}{}", "a".repeat(1_750_000), "é日".repeat(2_000));
        let chunks = split_evidence_packet(&packet);
        assert_eq!(chunks.concat(), packet);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.len() <= EVIDENCE_CHUNK_MAX_BYTES));
        assert!(EVIDENCE_CHUNK_MAX_BYTES < 2_000);
        let pages = evidence_page_inputs("evidence", "digest", packet.len(), &chunks);
        assert_eq!(pages.len(), chunks.len().div_ceil(EVIDENCE_CHUNKS_PER_PAGE));
        assert!(
            pages.len() > 18,
            "evidence paging must not reintroduce a fixed patch-size ceiling"
        );
        let mut reconstructed = Vec::new();
        for (page, input) in pages.iter().enumerate() {
            let input = input.as_object().unwrap();
            assert_eq!(input.len(), EVIDENCE_CHUNKS_PER_PAGE + 7);
            assert_eq!(input["page_key"], format!("evidence:{page:08}"));
            assert_eq!(input["page_index"], page.to_string());
            assert_eq!(input["page_count"], pages.len().to_string());
            assert_eq!(input["evidence_chunk_count"], chunks.len().to_string());
            assert_eq!(input["evidence_byte_count"], packet.len().to_string());
            assert!(serde_json::to_vec(input).unwrap().len() < 50 * 1024);
            for slot in 0..EVIDENCE_CHUNKS_PER_PAGE {
                let chunk = page * EVIDENCE_CHUNKS_PER_PAGE + slot;
                let value = input[&format!("evidence_chunk_{slot}")].as_str().unwrap();
                if chunk < chunks.len() {
                    reconstructed.push(value.to_owned());
                } else {
                    assert!(value.is_empty(), "only final page padding may be empty");
                }
            }
        }
        assert_eq!(reconstructed.concat(), packet);
    }

    #[test]
    fn empty_evidence_packet_has_no_rows() {
        let chunks = split_evidence_packet("");
        assert!(chunks.is_empty());
        assert!(evidence_page_inputs("empty", "digest", 0, &chunks).is_empty());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(run("not json").unwrap_err().contains("invalid JSON input"));
    }

    #[test]
    fn rejects_missing_focus() {
        let stdin = json!({
            "input": {},
            "nonce": "n",
            "host": {
                "git_diff": {
                    "repository": "/r", "base_sha": "b", "head_sha": "h",
                    "name_status": "", "stat": "", "patch": "",
                },
                "workspace": {
                    "workspace_id": "w", "owner_agent_did": "did:key:z", "authority": "readOnly",
                },
            },
        })
        .to_string();
        assert!(run(&stdin).unwrap_err().contains("input.focus"));
    }

    #[test]
    fn round_trips_a_minimal_envelope() {
        let stdin = json!({
            "input": {"focus": "look closely"},
            "nonce": "abc",
            "host": {
                "git_diff": {
                    "repository": "/r", "base_sha": "b", "head_sha": "h",
                    "name_status": "M\tfile.rs", "stat": "1 file changed", "patch": "diff --git",
                },
                "workspace": {
                    "workspace_id": "w1", "owner_agent_did": "did:key:zOwner", "authority": "readOnly",
                },
            },
        })
        .to_string();
        let output = run(&stdin).unwrap();
        assert_eq!(output.input["base_ref"], "b");
        assert_eq!(output.input["head_ref"], "h");
        assert_eq!(output.input["evidence_id"], "abc");
        assert_eq!(output.input["focus"], "look closely");
        assert_eq!(output.documents[0].collection, "CodeReviewEvidenceManifest");
        assert_eq!(output.documents[0].fields["evidence_id"], "abc");
        assert_eq!(
            output.documents.len(),
            2,
            "one manifest and one page for a short packet"
        );
        assert_eq!(output.documents[1].collection, "CodeReviewEvidencePage");
    }
}
