mod clean;
mod library;
mod timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Write},
    path::{Component, Path},
};
type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Span {
    source: String,
    page: u64,
    start_byte: usize,
    end_byte: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Block {
    id: String,
    markdown: String,
    sources: Vec<Span>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Section {
    id: String,
    title: String,
    level: u64,
    matter_type: String,
    blocks: Vec<Block>,
    #[serde(default)]
    chapter_key: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    entry_number: Option<String>,
    #[serde(default)]
    level_name: Option<String>,
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    audio_include: Option<bool>,
    #[serde(default)]
    audio_include_reasoning: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Chunk {
    key: String,
    title: String,
    blocks: Vec<Block>,
}
#[derive(Serialize, Deserialize)]
struct Plan {
    edition_id: String,
    book_id: String,
    title: String,
    author: String,
    language: String,
    modified: String,
    output: String,
    source: Value,
    chapters: Vec<Section>,
    chunks: Vec<Chunk>,
    mechanical_edits: Vec<Value>,
    #[serde(default)]
    dispatch_window: usize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    block_id: String,
    old_text: String,
    new_text: String,
    reason: String,
}
#[derive(Serialize, Deserialize)]
struct Reviewed {
    plan_hash: String,
    key: String,
    blocks: Vec<Block>,
    edits: Vec<Edit>,
}
fn hash(s: &[u8]) -> String {
    format!("{:x}", Sha256::digest(s))
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing {key}"))
}
fn name(s: &str) -> Result<()> {
    if s.is_empty()
        || s.contains(['/', '\\'])
        || !matches!(
            Path::new(s).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
    {
        Err("use a simple file name inside the bound folder".into())
    } else {
        Ok(())
    }
}
fn read(root: &Path, file: &str) -> Result<Vec<u8>> {
    name(file)?;
    let path = root.join(file);
    let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.len() > 32 * 1024 * 1024 {
        return Err("input must be a regular file below 32 MiB".into());
    }
    fs::read(path).map_err(|e| e.to_string())
}
fn save(root: &Path, file: &str, value: &Value) -> Result<()> {
    name(file)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let path = root.join(file);
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut f) => f
            .write_all(&bytes)
            .and_then(|_| f.sync_all())
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if read(root, file)? == bytes {
                Ok(())
            } else {
                Err(format!(
                    "{file} already contains a different edition; use a new edition_id"
                ))
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
fn load_plan(v: &Value, root: &Path) -> Result<(Plan, String)> {
    let bytes = read(root, field(v, "plan")?)?;
    let plan: Plan = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if plan.edition_id != field(v, "run_id")? || plan.book_id != field(v, "book_id")? {
        return Err("plan does not belong to this edition/book".into());
    }
    Ok((plan, hash(&bytes)))
}
fn prepare(v: &Value, root: &Path) -> Result<Value> {
    if !timestamp::timestamp(field(v, "modified")?) {
        return Err("modified must be a valid UTC timestamp YYYY-MM-DDTHH:MM:SSZ".into());
    }
    let source: Value =
        serde_json::from_slice(&read(root, field(v, "structured")?)?).map_err(|e| e.to_string())?;
    let edition = field(v, "run_id")?;
    let book = field(&source, "book_id")?;
    if field(v, "book_id")? != book {
        return Err("structured book identity does not match job".into());
    }
    if let Some(scope) = source.get("source_evidence") {
        let bytes = read(root, field(scope, "book_file")?)?;
        if hash(&bytes) != field(scope, "book_hash")? {
            return Err("source evidence changed before text review".into());
        }
        let capsule: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if capsule["run_id"] != scope["run_id"] || capsule["book_id"] != book {
            return Err("text review evidence belongs to another book/run".into());
        }
    }
    let output = field(v, "output")?;
    name(output)?;
    let mut plan = Plan {
        edition_id: edition.into(),
        book_id: book.into(),
        title: field(&source, "title")?.into(),
        author: field(&source, "author")?.into(),
        language: field(&source, "language")?.into(),
        modified: field(v, "modified")?.into(),
        output: output.into(),
        source: source.clone(),
        dispatch_window: 16,
        chapters: vec![],
        chunks: vec![],
        mechanical_edits: vec![],
    };
    let mut seen = BTreeSet::new();
    let vocabulary = clean::vocabulary(&source);
    let mut chapter_ids = BTreeMap::new();
    for (ci, ch) in source["chapters"]
        .as_array()
        .ok_or("missing chapters")?
        .iter()
        .enumerate()
    {
        let title = field(ch, "title")?;
        let pages = ch["pages"].as_array().ok_or("chapter missing pages")?;
        let level = ch["level"].as_u64().unwrap_or(1);
        let opening = if let Some(first) = pages.first() {
            (
                field(first, "source")?.to_owned(),
                first["page"].as_u64().ok_or("invalid opening page")?,
            )
        } else {
            let scan = ch["start_page"]
                .as_u64()
                .ok_or("empty container needs a verified opening")?;
            let mut offset = 0;
            let mut opening = None;
            for src in source["sources"]
                .as_array()
                .ok_or("empty container needs the source manifest")?
            {
                let count = src["page_count"].as_u64().ok_or("invalid source count")?;
                if scan > offset && scan <= offset + count {
                    opening = Some((field(src, "source")?.to_owned(), scan - offset));
                    break;
                }
                offset += count;
            }
            opening.ok_or("container opening outside source manifest")?
        };
        let id = format!(
            "s-{}",
            &hash(format!("{book}:{}:{}:{level}", opening.0, opening.1).as_bytes())[..20]
        );
        let chapter_key = ch["chapter_key"].as_str().map(str::to_owned);
        let parent_id = match ch["parent_key"].as_str() {
            Some(key) => Some(
                chapter_ids
                    .get(key)
                    .cloned()
                    .ok_or("chapter parent must precede its children")?,
            ),
            None => None,
        };
        if let Some(key) = &chapter_key {
            if chapter_ids.insert(key.clone(), id.clone()).is_some() {
                return Err("duplicate chapter key".into());
            }
        }
        for page in pages {
            let key = (
                field(page, "source")?.to_string(),
                page["page"].as_u64().ok_or("invalid page")?,
            );
            if !seen.insert(key) {
                return Err("duplicate source page".into());
            }
        }
        let blocks = clean::section(
            book,
            title,
            &plan.title,
            pages,
            &vocabulary,
            &mut plan.mechanical_edits,
        )?;
        let mut pending = vec![];
        let mut size = 0;
        for block in &blocks {
            if size + block.markdown.len() > 12000 && !pending.is_empty() {
                let key = format!("c{:04}", plan.chunks.len());
                plan.chunks.push(Chunk {
                    key,
                    title: title.into(),
                    blocks: std::mem::take(&mut pending),
                });
                size = 0;
            }
            if block.markdown.len() > 40000 {
                return Err(format!(
                    "section {ci} has a block over 40 KB; segment it before review"
                ));
            }
            pending.push(block.clone());
            size += block.markdown.len();
        }
        if !pending.is_empty() {
            let key = format!("c{:04}", plan.chunks.len());
            plan.chunks.push(Chunk {
                key,
                title: title.into(),
                blocks: pending,
            });
        }
        plan.chapters.push(Section {
            id,
            title: title.into(),
            level: ch["level"].as_u64().unwrap_or(1),
            matter_type: ch["matter_type"].as_str().unwrap_or("body").into(),
            blocks,
            chapter_key,
            parent_id,
            entry_number: ch["entry_number"].as_str().map(str::to_owned),
            level_name: ch["level_name"].as_str().map(str::to_owned),
            content_type: ch["content_type"].as_str().unwrap_or("").into(),
            audio_include: ch["audio_include"].as_bool(),
            audio_include_reasoning: ch["audio_include_reasoning"].as_str().unwrap_or("").into(),
        });
    }
    if seen.len() as u64 != source["page_count"].as_u64().ok_or("missing page count")?
        || plan.chunks.is_empty()
    {
        return Err("source coverage/count mismatch".into());
    }
    if plan.chunks.len() > 256 {
        return Err(
            "edition exceeds the runtime's 256-member fan-in limit; no plan published".into(),
        );
    }
    let file = format!("plan-{}.json", &hash(edition.as_bytes())[..20]);
    save(root, &file, &json!(plan))?;
    let path = v["path_original"].as_str().unwrap_or(field(v, "path")?);
    let chunks: Vec<Value> = plan
        .chunks
        .iter()
        .take(plan.dispatch_window)
        .map(|c| review_attempt(&plan, c, path, &file))
        .collect();
    Ok(
        json!({"plan":{"run_id":edition,"book_id":book,"path":path,"plan":file,"expected_total":plan.chunks.len()},"chunks":chunks}),
    )
}
fn review_attempt(plan: &Plan, chunk: &Chunk, path: &str, file: &str) -> Value {
    let attempt_id = format!("{}:{}:0", plan.edition_id, chunk.key);
    let mut out = json!({"attempt_id":attempt_id,"run_id":plan.edition_id,"book_id":plan.book_id,"path":path,"plan":file,
        "chunk_ref":chunk.key,"title":chunk.title,"attempt":"0","feedback":"",
        "blocks_json":serde_json::to_string(&chunk.blocks).unwrap(),
        "lane":if chunk.key.trim_start_matches('c').parse::<usize>().unwrap()%2==0{"reader"}else{"librarian"}});
    if let Some(scope) = plan.source.get("source_evidence") {
        out["source_run_id"] = scope["run_id"].clone();
        out["book_file"] = scope["book_file"].clone();
        out["book_hash"] = scope["book_hash"].clone();
        out["stage_job_id"] = out["attempt_id"].clone();
        out["image_command"] = json!("image");
        out["ocr_command"] = json!("ocr");
        let mut pages = BTreeMap::new();
        for block in &chunk.blocks {
            for span in &block.sources {
                let mut offset = 0;
                if let Some(sources) = plan.source["sources"].as_array() {
                    for src in sources {
                        if src["source"] == span.source {
                            pages.insert((span.source.clone(),span.page),json!({"source":span.source,"source_page":span.page,"scan_page":offset+span.page}));
                            break;
                        }
                        offset += src["page_count"].as_u64().unwrap_or(0);
                    }
                }
            }
        }
        out["source_pages_json"] =
            json!(serde_json::to_string(&pages.into_values().collect::<Vec<_>>()).unwrap());
    }
    out
}
fn quote_variant_source(source: &str, quoted: &str) -> Option<String> {
    fn fold(c: char) -> char {
        match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            c => c,
        }
    }
    let mut normalized = String::new();
    let mut offsets = BTreeMap::new();
    for (offset, c) in source.char_indices() {
        offsets.insert(normalized.len(), offset);
        normalized.push(fold(c));
    }
    offsets.insert(normalized.len(), source.len());
    let needle: String = quoted.chars().map(fold).collect();
    if needle.is_empty() {
        return None;
    }
    let mut matches = normalized.match_indices(&needle);
    let (start, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(source[*offsets.get(&start)?..*offsets.get(&(start + needle.len()))?].into())
}

fn validate_review(chunk: &Chunk, key: &str, digest: String, raw: &str) -> Result<Reviewed> {
    let edits: Vec<Edit> = serde_json::from_str(raw).map_err(|e| format!("edits_json: {e}"))?;
    if edits.len() > 50 {
        return Err("at most 50 focused edits are allowed per review".into());
    }
    let mut reviewed = Reviewed {
        plan_hash: digest,
        key: key.into(),
        blocks: chunk.blocks.clone(),
        edits: vec![],
    };
    for (index, edit) in edits.into_iter().enumerate() {
        if edit.old_text.is_empty()
            || edit.old_text.len() > 1200
            || edit.new_text.len() > 2400
            || edit.reason.trim().is_empty()
            || edit.reason.len() > 1200
        {
            return Err("edits must be small, exact, and reasoned".into());
        }
        let b = reviewed
            .blocks
            .iter_mut()
            .find(|b| b.id == edit.block_id)
            .ok_or("edit names a block outside this chunk")?;
        let matches = b.markdown.matches(&edit.old_text).count();
        if matches != 1 {
            let recovery = if matches == 0 {
                match quote_variant_source(&b.markdown, &edit.old_text) {
                    Some(source) => format!(
                        "The uniquely located quote variant in the source is {}. Copy its punctuation exactly in old_text, or omit this edit.",
                        json!(source)
                    ),
                    None => {
                        "Quote old_text exactly from the original block, or omit this edit.".into()
                    }
                }
            } else {
                "Include surrounding source text in old_text to locate one occurrence, or omit this edit.".into()
            };
            return Err(format!(
                "Edit {} for {}: old_text {} matches {matches} times; it must match exactly once. {recovery} No edits committed.",
                index + 1,
                edit.block_id,
                json!(edit.old_text)
            ));
        }
        let next = b.markdown.replacen(&edit.old_text, &edit.new_text, 1);
        if next.trim().is_empty() {
            return Err("do not delete an entire passage; flag uncertain OCR for review".into());
        }
        b.markdown = next;
        reviewed.edits.push(edit);
    }
    Ok(reviewed)
}
#[derive(Debug)]
enum ApplyFailure {
    Edit(String),
    Execution(String),
}
impl From<String> for ApplyFailure {
    fn from(error: String) -> Self {
        Self::Execution(error)
    }
}
impl From<&str> for ApplyFailure {
    fn from(error: &str) -> Self {
        Self::Execution(error.into())
    }
}
fn apply(v: &Value, root: &Path) -> std::result::Result<Value, ApplyFailure> {
    let (plan, digest) = load_plan(v, root)?;
    let key = field(v, "chunk_ref")?;
    let chunk = plan
        .chunks
        .iter()
        .find(|c| c.key == key)
        .ok_or("unknown polish chunk")?;
    let file = format!("{}-{key}.json", field(v, "plan")?.trim_end_matches(".json"));
    // The file is the durable acceptance checkpoint. Replaying it must also
    // replay the receipt after a crash between the file and database commits.
    let reviewed = match fs::symlink_metadata(root.join(&file)) {
        Ok(_) => {
            let accepted: Reviewed =
                serde_json::from_slice(&read(root, &file)?).map_err(|e| e.to_string())?;
            if accepted.plan_hash != digest
                || accepted.key != key
                || accepted.blocks.len() != chunk.blocks.len()
                || accepted
                    .blocks
                    .iter()
                    .zip(&chunk.blocks)
                    .any(|(a, b)| a.id != b.id || a.sources != b.sources)
            {
                return Err("accepted checkpoint does not match the immutable plan".into());
            }
            accepted
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let candidate = validate_review(chunk, key, digest, field(v, "edits_json")?)
                .map_err(ApplyFailure::Edit)?;
            save(root, &file, &json!(candidate))?;
            candidate
        }
        Err(e) => return Err(e.to_string().into()),
    };
    let file = format!("{}-{key}.json", field(v, "plan")?.trim_end_matches(".json"));
    Ok(
        json!({"receipt_id":format!("{}:{key}",plan.edition_id),"run_id":plan.edition_id,"book_id":plan.book_id,"path":v["path_original"].as_str().unwrap_or(field(v,"path")?),"plan":v["plan"],"chunk_ref":key,"expected_total":plan.chunks.len(),"sha256":hash(&read(root,&file)?),"edit_count":reviewed.edits.len()}),
    )
}
fn apply_with_repair(v: &Value, root: &Path) -> Result<Value> {
    match apply(v, root) {
        Ok(chunk) => {
            let (plan, _) = load_plan(v, root)?;
            let next = if plan.dispatch_window == 0 {
                None
            } else {
                let index = plan
                    .chunks
                    .iter()
                    .position(|c| c.key == v["chunk_ref"])
                    .ok_or("unknown review chunk")?;
                plan.chunks.get(index + plan.dispatch_window).map(|next| {
                    review_attempt(
                        &plan,
                        next,
                        v["path_original"]
                            .as_str()
                            .unwrap_or(v["path"].as_str().unwrap()),
                        v["plan"].as_str().unwrap(),
                    )
                })
            };
            Ok(json!({"chunk":chunk,"retry":next,"failure":null}))
        }
        Err(ApplyFailure::Execution(error)) => Err(error),
        Err(ApplyFailure::Edit(error)) => {
            let (plan, _) = load_plan(v, root)?;
            let key = field(v, "chunk_ref")?;
            let chunk = plan
                .chunks
                .iter()
                .find(|c| c.key == key)
                .ok_or("unknown polish chunk")?;
            let attempt = field(v, "attempt")?
                .parse::<usize>()
                .map_err(|_| "invalid attempt")?;
            let path = v["path_original"].as_str().unwrap_or(field(v, "path")?);
            if attempt >= 2 {
                return Ok(
                    json!({"chunk":null,"retry":null,"failure":{"run_id":plan.edition_id,"book_id":plan.book_id,"chunk_ref":key,"error":error}}),
                );
            }
            let mut retry = review_attempt(&plan, chunk, path, field(v, "plan")?);
            retry["attempt"] = json!((attempt + 1).to_string());
            retry["attempt_id"] = json!(format!("{}:{key}:{}", plan.edition_id, attempt + 1));
            if retry.get("stage_job_id").is_some() {
                retry["stage_job_id"] = retry["attempt_id"].clone();
            }
            retry["feedback"] = json!(error);
            Ok(json!({"chunk":null,"failure":null,"retry":retry}))
        }
    }
}

fn human_book(plan: &Plan) -> Result<Value> {
    fn section(ch: &Section, all: &[Section], depth: usize) -> Result<Value> {
        if depth > 6 {
            return Err("chapter hierarchy exceeds six levels".into());
        }
        let children: Vec<_> = all
            .iter()
            .filter(|c| c.parent_id.as_deref() == Some(&ch.id))
            .map(|c| section(c, all, depth + 1))
            .collect::<Result<_>>()?;
        let paragraphs: Vec<_> = ch.blocks.iter().enumerate().map(|(i,b)| json!({
            "id":b.id,"ordinal":i+1,"kind":if b.markdown.trim_start().starts_with('#'){"heading"}else{"paragraph"},
            "text":b.markdown,"source_spans":b.sources,"epub_href":format!("OEBPS/{}.xhtml#{}",ch.id,b.id)
        })).collect();
        Ok(
            json!({"id":ch.id,"title":ch.title,"entry_number":ch.entry_number,"level":ch.level,
            "level_name":ch.level_name,"matter_type":ch.matter_type,"content_type":ch.content_type,
            "audio_include":ch.audio_include,"audio_include_reasoning":ch.audio_include_reasoning,
            "paragraphs":paragraphs,"sections":children}),
        )
    }
    let sections: Vec<_> = plan
        .chapters
        .iter()
        .filter(|ch| ch.parent_id.is_none())
        .map(|ch| section(ch, &plan.chapters, 1))
        .collect::<Result<_>>()?;
    Ok(
        json!({"id":plan.book_id,"edition_id":plan.edition_id,"title":plan.title,"author":plan.author,
        "language":plan.language,"metadata":plan.source["metadata"],"sections":sections}),
    )
}
fn finish(v: &Value, root: &Path) -> Result<Value> {
    let (mut plan, digest) = load_plan(v, root)?;
    let mut blocks = BTreeMap::new();
    let mut edits = vec![];
    for c in &plan.chunks {
        let file = format!(
            "{}-{}.json",
            field(v, "plan")?.trim_end_matches(".json"),
            c.key
        );
        let reviewed: Reviewed =
            serde_json::from_slice(&read(root, &file)?).map_err(|e| e.to_string())?;
        if reviewed.plan_hash != digest
            || reviewed.key != c.key
            || reviewed.blocks.iter().map(|b| &b.id).collect::<Vec<_>>()
                != c.blocks.iter().map(|b| &b.id).collect::<Vec<_>>()
        {
            return Err("reviewed chunk does not match its immutable plan".into());
        }
        for (actual, original) in reviewed.blocks.iter().zip(&c.blocks) {
            if actual.sources != original.sources {
                return Err("polish changed provenance".into());
            }
        }
        edits.extend(reviewed.edits);
        for b in reviewed.blocks {
            if blocks.insert(b.id.clone(), b).is_some() {
                return Err("duplicate passage ID".into());
            }
        }
    }
    let mut passages = vec![];
    for ch in &mut plan.chapters {
        for b in &mut ch.blocks {
            *b = blocks.remove(&b.id).ok_or("missing reviewed passage")?;
            passages.push(json!({"run_id":plan.edition_id,"book_id":plan.book_id,"edition_id":plan.edition_id,"passage_id":b.id,"chapter_id":ch.id,"chapter_title":ch.title,"markdown":b.markdown,"source_spans_json":serde_json::to_string(&b.sources).unwrap(),"epub_href":format!("OEBPS/{}.xhtml#{}",ch.id,b.id)}));
        }
    }
    if !blocks.is_empty() {
        return Err("unassigned reviewed passages".into());
    }
    let prefix = field(v, "plan")?.trim_end_matches(".json");
    let manuscript = format!("{prefix}-manuscript.json");
    let structured = format!("{prefix}-book.json");
    let human = human_book(&plan)?;
    let edition = json!({"identifier":plan.edition_id,"title":plan.title,"author":plan.author,"language":plan.language,"modified":plan.modified,"chapters":plan.chapters});
    save(root, &manuscript, &edition)?;
    save(
        root,
        &structured,
        &json!({"book":human,"edition":edition,"original":plan.source,"mechanical_edits":plan.mechanical_edits,"polish_edits":edits,"passages":passages}),
    )?;
    let path = v["path_original"].as_str().unwrap_or(field(v, "path")?);
    Ok(
        json!({"prepared":{"run_id":plan.edition_id,"book_id":plan.book_id,"edition_id":plan.edition_id,"path":path,"structured_file":structured,"manuscript":manuscript,"passage_count":passages.len(),"edit_count":edits.len()},"passages":passages,"export":{"run_id":plan.edition_id,"book_id":plan.book_id,"edition_id":plan.edition_id,"path":path,"manuscript":manuscript,"output":plan.output,"structured_file":structured}}),
    )
}
fn run(v: Value) -> Result<Value> {
    let root = Path::new(field(&v, "path")?);
    if !root.is_dir() {
        return Err("bound folder unavailable".into());
    }
    if v.get("structured_file").is_some() {
        library::index(&v, root)
    } else if v.get("structured").is_some() {
        prepare(&v, root)
    } else if v.get("edits_json").is_some() {
        apply_with_repair(&v, root)
    } else {
        finish(&v, root)
    }
}
fn main() {
    let result = (|| {
        let mut raw = String::new();
        io::stdin()
            .take(4_000_001)
            .read_to_string(&mut raw)
            .map_err(|e| e.to_string())?;
        if raw.len() > 4_000_000 {
            return Err("input exceeds 4 MB".into());
        }
        run(serde_json::from_str(&raw).map_err(|e| e.to_string())?)
    })();
    match result {
        Ok(v) => {
            serde_json::to_writer(io::stdout().lock(), &v).unwrap();
        }
        Err(e) => {
            eprintln!("shelf_prepare: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_opening_part_and_chapter_remain_hierarchical_narration_units() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = json!({"book_id":"book-one","title":"Unfamiliar Book","author":"Author","language":"en","page_count":3,"sources":[{"source":"scan.pdf","page_count":3}],"chapters":[
            {"chapter_key":"run:part","title":"Part I","entry_number":"I","level_name":"part","level":1,"matter_type":"body","content_type":"body","audio_include":true,"audio_include_reasoning":"Announce the part heading.","start_page":1,"pages":[]},
            {"chapter_key":"run:first","parent_key":"run:part","title":"First chapter","entry_number":"1","level_name":"chapter","level":2,"matter_type":"body","content_type":"body","audio_include":true,"audio_include_reasoning":"Narrative prose.","pages":[{"source":"scan.pdf","page":1,"markdown":"Opening paragraph."},{"source":"scan.pdf","page":2,"markdown":"Continuation paragraph."}]},
            {"chapter_key":"run:notes","title":"Notes","level_name":"notes","level":1,"matter_type":"back_matter","content_type":"notes","audio_include":false,"audio_include_reasoning":"Reference citations.","pages":[{"source":"scan.pdf","page":3,"markdown":"1. A reference."}]}
        ]});
        save(root, "source.json", &source).unwrap();
        let prepared=run(json!({"run_id":"edition","book_id":"book-one","path":root,"structured":"source.json","modified":"2026-10-09T00:00:00Z","output":"book.epub"})).unwrap();
        let chunks = prepared["chunks"].as_array().unwrap();
        for chunk in chunks {
            let mut job = chunk.clone();
            job["edits_json"] = json!("[]");
            apply(&job, root).unwrap();
        }
        let out = finish(&chunks[0], root).unwrap();
        let snapshot: Value = serde_json::from_slice(
            &read(root, out["prepared"]["structured_file"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let book = &snapshot["book"];
        assert_eq!(book["sections"].as_array().unwrap().len(), 2);
        assert!(
            book["sections"][0]["paragraphs"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let chapter = &book["sections"][0]["sections"][0];
        assert_eq!(chapter["entry_number"], "1");
        assert_eq!(chapter["audio_include"], true);
        assert_eq!(book["sections"][1]["audio_include"], false);
        assert_eq!(
            chapter["paragraphs"][0]["id"],
            snapshot["passages"][0]["passage_id"]
        );
        assert_eq!(
            chapter["paragraphs"][0]["epub_href"],
            snapshot["passages"][0]["epub_href"]
        );
        assert_ne!(
            snapshot["edition"]["chapters"][0]["id"],
            snapshot["edition"]["chapters"][1]["id"]
        );
        assert_eq!(snapshot["original"], source);
    }
    #[test]
    fn quote_variant_feedback_never_applies_an_inexact_edit() {
        let source = "Élodie’s bi- ography includes “road-map”.";
        let block = Block {
            id: "passage".into(),
            markdown: source.into(),
            sources: vec![Span {
                source: "fixture.pdf".into(),
                page: 2,
                start_byte: 0,
                end_byte: source.len(),
            }],
        };
        let chunk = Chunk {
            key: "c0000".into(),
            title: "One".into(),
            blocks: vec![block.clone()],
        };
        let mut edit = json!({"block_id":"passage", "old_text":"Élodie's bi- ography", "new_text":"Élodie’s biography", "reason":"Join a split word"});
        let error = validate_review(
            &chunk,
            &chunk.key,
            "hash".into(),
            &json!([edit]).to_string(),
        )
        .err()
        .unwrap();
        assert!(error.contains("Edit 1"));
        assert!(error.contains("matches 0 times"));
        assert!(error.contains("Élodie’s bi- ography"));
        assert_eq!(chunk.blocks[0].markdown, source);
        edit["old_text"] = json!("Élodie’s bi- ography");
        let reviewed = validate_review(
            &chunk,
            &chunk.key,
            "hash".into(),
            &json!([edit]).to_string(),
        )
        .unwrap();
        assert_eq!(
            reviewed.blocks[0].markdown,
            "Élodie’s biography includes “road-map”."
        );
        assert_eq!(reviewed.blocks[0].sources, block.sources);
        assert_eq!(
            quote_variant_source(source, "\"road-map\""),
            Some("“road-map”".into())
        );
        assert_eq!(quote_variant_source("child’s and child's", "child's"), None);
        assert_eq!(quote_variant_source(source, "invented text"), None);
    }

    #[test]
    fn bounded_dispatch_releases_one_successor_only_after_an_accepted_review() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let chapters: Vec<Value> = (1..=17)
            .map(|page| {
                json!({
                    "title":format!("Chapter {page}"),"level":1,"matter_type":"body",
                    "pages":[{"source":"fixture.pdf","page":page,"markdown":"A paragraph."}]
                })
            })
            .collect();
        save(
            root,
            "source.json",
            &json!({"book_id":"b","title":"Book","author":"Author",
            "language":"en","page_count":17,"chapters":chapters}),
        )
        .unwrap();
        let prepared = run(json!({"run_id":"edition","book_id":"b","path":root,
            "structured":"source.json","modified":"2026-10-08T00:00:00Z","output":"book.epub"}))
        .unwrap();
        assert_eq!(prepared["chunks"].as_array().unwrap().len(), 16);
        assert_eq!(prepared["plan"]["expected_total"], 17);
        let mut first = prepared["chunks"][0].clone();
        first["edits_json"] = json!("[]");
        let accepted = run(first.clone()).unwrap();
        assert_eq!(accepted["chunk"]["expected_total"], 17);
        assert_eq!(accepted["retry"]["chunk_ref"], "c0016");
        assert_eq!(run(first.clone()).unwrap(), accepted);
        first["edits_json"] = json!("a conflicting duplicate must not replace accepted text");
        assert_eq!(run(first).unwrap(), accepted);
        let mut missing = prepared["chunks"][1].clone();
        missing["plan"] = json!("absent.json");
        missing["edits_json"] = json!("[]");
        assert!(
            run(missing).is_err(),
            "infrastructure failure must not create a model repair"
        );
        let mut rejected = prepared["chunks"][1].clone();
        rejected["edits_json"] = json!("invalid");
        let repair = run(rejected).unwrap();
        assert!(repair["chunk"].is_null());
        assert_eq!(repair["retry"]["chunk_ref"], "c0001");
        assert_eq!(repair["retry"]["attempt"], "1");
        let mut last = accepted["retry"].clone();
        last["edits_json"] = json!("[]");
        assert!(run(last).unwrap()["retry"].is_null());
    }
    #[test]
    fn edition_requires_every_review_and_keeps_source_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let book = json!({"book_id":"book-1","title":"Example","author":"Author","language":"en","page_count":2,"chapters":[{"title":"One","level":1,"matter_type":"body","pages":[{"source":"one.pdf","page":1,"markdown":"A mistkae in a paragraph.\n\n1"}]},{"title":"Two","level":1,"matter_type":"body","pages":[{"source":"one.pdf","page":2,"markdown":"A second paragraph.\n\n2"}]}]});
        save(root, "source.json", &book).unwrap();
        let request = json!({"run_id":"edition-1","book_id":"book-1","path":root,"structured":"source.json","modified":"2026-10-08T00:00:00Z","output":"book.epub"});
        let mut invalid = request.clone();
        invalid["modified"] = json!("2026-02-30T00:00:00Z");
        assert!(run(invalid).is_err());
        let prepared = run(request.clone()).unwrap();
        assert_eq!(run(request).unwrap(), prepared);
        let chunks = prepared["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 2);
        let finish_request = chunks[0].clone();
        assert!(run(finish_request.clone()).is_err());
        for (i, c) in chunks.iter().enumerate() {
            let mut job = c.clone();
            let blocks: Vec<Block> =
                serde_json::from_str(c["blocks_json"].as_str().unwrap()).unwrap();
            let edits = if i == 0 {
                json!([{"block_id":blocks[0].id,"old_text":"mistkae","new_text":"mistake","reason":"OCR transposition"}])
            } else {
                json!([])
            };
            job["edits_json"] = json!(edits.to_string());
            let result = apply(&job, root).unwrap();
            assert_eq!(apply(&job, root).unwrap(), result);
        }
        let completed = run(finish_request).unwrap();
        assert_eq!(completed["prepared"]["passage_count"], 2);
        assert_eq!(
            completed["passages"][0]["markdown"],
            "A mistake in a paragraph."
        );
        assert_eq!(
            completed["passages"][0]["passage_id"],
            serde_json::from_str::<Value>(chunks[0]["blocks_json"].as_str().unwrap()).unwrap()[0]["id"]
        );
        let snapshot: Value = serde_json::from_slice(
            &read(
                root,
                completed["prepared"]["structured_file"].as_str().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(snapshot["original"], book);
        let mut index_request = completed["prepared"].clone();
        index_request["path"] = json!(root);
        let indexed = run(index_request.clone()).unwrap();
        assert_eq!(indexed["edition"]["access"], "local_only");
        assert_eq!(indexed["edition"]["passage_count"], 2);
        assert_eq!(indexed["passages"][0]["text"], "A mistake in a paragraph.");
        assert_eq!(
            indexed["passages"][0]["preview"],
            "A mistake in a paragraph."
        );
        assert_eq!(
            indexed["passages"][0]["locator_summary"],
            "one.pdf, PDF page 1"
        );
        assert_eq!(
            indexed["passages"][0]["text_hash"],
            hash(b"A mistake in a paragraph.")
        );
        assert_eq!(run(index_request.clone()).unwrap(), indexed);
        let mut tampered = snapshot.clone();
        tampered["passages"][0]["markdown"] = json!("Text absent from the reviewed edition.");
        save(root, "tampered.json", &tampered).unwrap();
        let mut bad_index = index_request.clone();
        bad_index["structured_file"] = json!("tampered.json");
        assert!(run(bad_index).is_err());
        index_request["book_id"] = json!("another-book");
        assert!(run(index_request).is_err());
        let mut bad = chunks[0].clone();
        bad["edits_json"] = json!(
            "[{\"block_id\":\"unknown\",\"old_text\":\"a\",\"new_text\":\"b\",\"reason\":\"bad\"}]"
        );
        assert!(apply(&bad, root).is_ok());
    }
}
