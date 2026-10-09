use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path},
};

pub type Result<T> = std::result::Result<T, String>;
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn field<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v[name]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing {name}"))
}
pub fn filename(s: &str) -> Result<()> {
    if s.is_empty()
        || s.contains(['/', '\\'])
        || !matches!(
            Path::new(s).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
    {
        return Err("use a simple file name in the bound book folder".into());
    }
    Ok(())
}
pub fn read(root: &Path, name: &str, limit: u64) -> Result<Vec<u8>> {
    filename(name)?;
    let path = root.join(name);
    let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.len() > limit {
        return Err("book input must be a regular file within its size limit".into());
    }
    fs::read(path).map_err(|e| e.to_string())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub source: String,
    pub page_count: u32,
    pub asset: String,
    pub sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub scan_page: u32,
    pub source: String,
    pub page: u32,
    pub markdown: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookInput {
    pub run_id: String,
    pub book_id: String,
    pub sources: Vec<Source>,
    pub pages: Vec<Page>,
    pub access: String,
    pub license: String,
}
impl BookInput {
    pub fn load(root: &Path, v: &Value) -> Result<Self> {
        let bytes = read(root, field(v, "book_file")?, 32 * 1024 * 1024)?;
        if hash(&bytes) != field(v, "book_hash")? {
            return Err("book evidence changed; create a new discovery run".into());
        }
        let book: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if book.run_id != field(v, "run_id")? || book.book_id != field(v, "book_id")? {
            return Err("evidence belongs to another book/run".into());
        }
        let mut names = BTreeSet::new();
        let mut offset = 0;
        for source in &book.sources {
            if source.page_count == 0 || !names.insert(&source.source) {
                return Err("invalid source manifest".into());
            }
            filename(&source.asset)?;
            for physical in 1..=source.page_count {
                let scan = offset + physical;
                let page = book
                    .pages
                    .get((scan - 1) as usize)
                    .ok_or("source page missing")?;
                if page.scan_page != scan || page.source != source.source || page.page != physical {
                    return Err("source pages do not follow the exact manifest".into());
                }
            }
            offset = offset
                .checked_add(source.page_count)
                .ok_or("page count overflow")?;
        }
        if offset == 0 || book.pages.len() != offset as usize {
            return Err("source page coverage mismatch".into());
        }
        Ok(book)
    }
    pub fn page(&self, scan: u32) -> Result<&Page> {
        self.pages
            .get(scan.checked_sub(1).ok_or("pages start at one")? as usize)
            .ok_or("scan page outside this book".into())
    }
    pub fn headings(&self) -> Vec<Value> {
        self.pages.iter().flat_map(|p|p.markdown.lines().filter_map(move |line|{
            let level=line.chars().take_while(|c|*c=='#').count();
            if !(1..=2).contains(&level) || !line.as_bytes().get(level).is_some_and(u8::is_ascii_whitespace) {return None;}
            Some(serde_json::json!({"scan_page":p.scan_page,"heading":{"text":line[level..].trim(),"level":level},"source":p.source,"source_page":p.page}))
        })).collect()
    }
}
