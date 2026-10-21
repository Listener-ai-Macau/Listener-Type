//! Explicit, immutable text drafts. Separate from speech History and audio retention.
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::{Mutex, OnceLock}};
const LIMIT: usize = 50;
const FILE_LIMIT: usize = 4 * 1024 * 1024;
static ACCESS: OnceLock<Mutex<()>> = OnceLock::new();
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedTextDraft {
    pub id: String,
    pub created_at: String,
    pub name: String,
    pub original: String,
    pub text: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Library { schema: u8, drafts: Vec<SavedTextDraft> }
fn valid_content(id: &str, name: &str, original: &str, text: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok() && !name.trim().is_empty() && name.chars().count() <= 80
        && !text.trim().is_empty() && text.chars().count() <= 100_000 && original.chars().count() <= 100_000
}
fn read(path: &Path) -> Result<Library, String> {
    if !path.exists() { return Ok(Library { schema: 1, drafts: vec![] }); }
    if fs::metadata(path).map_err(|_| "saved_draft_read")?.len() > FILE_LIMIT as u64 { return Err("saved_draft_file_limit".into()); }
    let bytes = fs::read(path).map_err(|_| "saved_draft_read")?;
    if bytes.len() > FILE_LIMIT { return Err("saved_draft_file_limit".into()); }
    let lib: Library = serde_json::from_slice(&bytes).map_err(|_| "saved_draft_invalid_file")?;
    let mut ids = std::collections::HashSet::new();
    if lib.schema != 1 || lib.drafts.len() > LIMIT || lib.drafts.iter().any(|d|
        !valid_content(&d.id, &d.name, &d.original, &d.text) || !ids.insert(d.id.clone())
        || chrono::DateTime::parse_from_rfc3339(&d.created_at).is_err()) { return Err("saved_draft_invalid_file".into()); }
    Ok(lib)
}
fn write(path: &Path, lib: &Library) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(lib).map_err(|_| "saved_draft_write")?;
    if bytes.len() > FILE_LIMIT { return Err("saved_draft_file_limit".into()); }
    crate::persistence::write_saved_text_drafts(path, &bytes).map_err(|_| "saved_draft_write".into())
}
pub fn list(path: &Path) -> Result<Vec<SavedTextDraft>, String> {
    let _lock = ACCESS.get_or_init(|| Mutex::new(())).lock().map_err(|_| "saved_draft_busy")?;
    Ok(read(path)?.drafts)
}
pub fn save(path: &Path, id: String, name: String, original: String, text: String) -> Result<SavedTextDraft, String> {
    let _lock = ACCESS.get_or_init(|| Mutex::new(())).lock().map_err(|_| "saved_draft_busy")?;
    if !valid_content(&id, &name, &original, &text) { return Err("saved_draft_invalid_text".into()); }
    let mut lib = read(path)?;
    if let Some(old) = lib.drafts.iter().find(|d| d.id == id) {
        return if old.name == name && old.original == original && old.text == text { Ok(old.clone()) }
            else { Err("saved_draft_changed".into()) };
    }
    if lib.drafts.len() >= LIMIT { return Err("saved_draft_limit".into()); }
    let draft = SavedTextDraft { id, created_at: chrono::Utc::now().to_rfc3339(), name, original, text };
    lib.drafts.insert(0, draft.clone()); write(path, &lib)?; Ok(draft)
}
pub fn delete(path: &Path, id: &str) -> Result<(), String> {
    let _lock = ACCESS.get_or_init(|| Mutex::new(())).lock().map_err(|_| "saved_draft_busy")?;
    let mut lib = read(path)?;
    lib.drafts.retain(|d| d.id != id); write(path, &lib)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_drafts_preserve_edits_and_retries_without_duplication() {
        let root=std::env::temp_dir().join(format!("listener-text-drafts-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();let p=root.join("drafts.json");let id=uuid::Uuid::new_v4().to_string();
        let a=save(&p,id.clone(),"Light".into(),"Original condition".into(),"Reviewed — 条件😀\nKeep it".into()).unwrap();
        assert_eq!(save(&p,id.clone(),a.name.clone(),a.original.clone(),a.text.clone()).unwrap(),a);
        assert!(save(&p,id.clone(),a.name.clone(),a.original.clone(),"Changed".into()).is_err());assert_eq!(list(&p).unwrap(),vec![a]);
        delete(&p,&id).unwrap();assert!(list(&p).unwrap().is_empty());fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn bad_files_and_full_library_never_discard_existing_drafts() {
        let root=std::env::temp_dir().join(format!("listener-text-drafts-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();let p=root.join("drafts.json");fs::write(&p,b"broken").unwrap();
        assert!(save(&p,uuid::Uuid::new_v4().to_string(),"A".into(),"".into(),"Keep".into()).is_err());assert_eq!(fs::read(&p).unwrap(),b"broken");fs::remove_file(&p).unwrap();
        assert!(save(&p,"bad-id".into(),"A".into(),"".into(),"Keep".into()).is_err());assert!(save(&p,uuid::Uuid::new_v4().to_string(),"A".into(),"".into(),"x".repeat(100_001)).is_err());
        for _ in 0..LIMIT { save(&p,uuid::Uuid::new_v4().to_string(),"A".into(),"".into(),"Keep".into()).unwrap(); }
        assert_eq!(save(&p,uuid::Uuid::new_v4().to_string(),"A".into(),"".into(),"Extra".into()).unwrap_err(),"saved_draft_limit");assert_eq!(list(&p).unwrap().len(),LIMIT);fs::remove_dir_all(root).unwrap();
    }
}
