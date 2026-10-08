use super::*;

pub(super) fn index(v: &Value, root: &Path) -> Result<Value> {
    let raw = read(root, field(v, "structured_file")?)?;
    let book: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let edition = &book["edition"];
    let source = &book["original"];
    let book_id = field(v, "book_id")?;
    let edition_id = field(v, "run_id")?;
    if field(source, "book_id")? != book_id
        || field(edition, "identifier")? != edition_id
        || field(v, "edition_id")? != edition_id
    {
        return Err("reviewed artifact does not belong to this book/edition".into());
    }
    let source_hash = hash(&serde_json::to_vec(source).map_err(|e| e.to_string())?);
    let revision = hash(&raw);
    let access = source["access"].as_str().unwrap_or("local_only");
    if !matches!(access, "open" | "local_only" | "restricted") {
        return Err("access must be open, local_only, or restricted".into());
    }
    let license = source["license"].as_str().unwrap_or("unknown");
    if access == "open" && (license.trim().is_empty() || license == "unknown") {
        return Err("open access requires an explicit license".into());
    }
    let mut expected = BTreeMap::new();
    for chapter in edition["chapters"]
        .as_array()
        .ok_or("missing reviewed chapters")?
    {
        for block in chapter["blocks"]
            .as_array()
            .ok_or("missing reviewed blocks")?
        {
            let id = field(block, "id")?;
            if expected.insert(id, (chapter, block)).is_some() {
                return Err("duplicate canonical passage".into());
            }
        }
    }
    let mut rows = vec![];
    for passage in book["passages"]
        .as_array()
        .ok_or("missing reviewed passages")?
    {
        let id = field(passage, "passage_id")?;
        let (chapter, block) = expected.remove(id).ok_or("unknown or duplicate passage")?;
        let spans: Value = serde_json::from_str(field(passage, "source_spans_json")?)
            .map_err(|e| e.to_string())?;
        let href = format!("OEBPS/{}.xhtml#{id}", field(chapter, "id")?);
        if passage["markdown"] != block["markdown"]
            || spans != block["sources"]
            || passage["epub_href"] != href
            || passage["edition_id"] != edition_id
            || passage["book_id"] != book_id
        {
            return Err("search passage differs from the reviewed edition".into());
        }
        let text = block["markdown"].as_str().ok_or("missing passage text")?;
        if text.trim().is_empty() {
            continue;
        }
        let record = hash(&serde_json::to_vec(&(book_id, edition_id, id)).unwrap());
        rows.push(json!({
            "record_id":record,"run_id":edition_id,"book_id":book_id,"edition_id":edition_id,
            "passage_id":id,"chapter_id":chapter["id"],"chapter_title":chapter["title"],
            "title":edition["title"],"author":edition["author"],"language":edition["language"],
            "access":access,"license":license,"source_type":"scan_pages",
            "source_hash":source_hash,"source_hash_scope":"structured_source",
            "text_hash":hash(text.as_bytes()),"revision":revision,"status":"reviewed",
            "analyzer":"english","text":text,"source_spans_json":passage["source_spans_json"],
            "epub_href":href
        }));
    }
    if !expected.is_empty() || rows.is_empty() {
        return Err("search index does not cover the reviewed edition".into());
    }
    Ok(json!({"edition":{
        "run_id":edition_id,"book_id":book_id,"edition_id":edition_id,
        "title":edition["title"],"author":edition["author"],"language":edition["language"],
        "access":access,"license":license,"source_hash":source_hash,
        "source_hash_scope":"structured_source","revision":revision,"modified":edition["modified"],
        "passage_count":rows.len(),"status":"reviewed"
    },"passages":rows}))
}
