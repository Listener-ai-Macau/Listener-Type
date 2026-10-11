//! Publish a protected voice profile only after every model-specific entry is
//! present. One small commit credential owns the active generation. Partial
//! enrollment never replaces or deletes the previously active voiceprint.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ACTIVE: &str = "owner-profile-active-v5";
const MAX_ENTRIES: usize = 8;

// Windows stores passwords as UTF-16 but voice profiles are serialized data.
// Bind the binary encoding explicitly; old password credentials remain readable.
const WINDOWS_BINARY_PREFIX: &[u8] = b"ListenerVoiceProfile\x01";
const WINDOWS_CREDENTIAL_LIMIT_BYTES: usize = 2560;

pub(crate) fn encode_windows_credential(value: &str) -> Result<Vec<u8>, String> {
    let size = WINDOWS_BINARY_PREFIX.len() + value.len();
    if size > WINDOWS_CREDENTIAL_LIMIT_BYTES {
        return Err(format!("protected voice profile needs {size} bytes; Windows limit is {WINDOWS_CREDENTIAL_LIMIT_BYTES} bytes"));
    }
    let mut blob = Vec::with_capacity(size);
    blob.extend_from_slice(WINDOWS_BINARY_PREFIX);
    blob.extend_from_slice(value.as_bytes());
    Ok(blob)
}

pub(crate) fn decode_windows_credential(blob: &[u8]) -> Result<String, String> {
    if let Some(value) = blob.strip_prefix(WINDOWS_BINARY_PREFIX) {
        return String::from_utf8(value.to_vec())
            .map_err(|_| "protected voice profile UTF-8 is corrupt".into());
    }
    if blob.len() % 2 != 0 {
        return Err("legacy protected voice profile UTF-16 is corrupt".into());
    }
    let units = blob.chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&units)
        .map_err(|_| "legacy protected voice profile UTF-16 is corrupt".into())
}

pub(crate) trait CredentialStore {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

#[derive(Serialize, Deserialize)]
struct Commit {
    version: u8,
    generation: uuid::Uuid,
    hashes: Vec<String>,
}

fn account(generation: uuid::Uuid, index: usize) -> String {
    format!("owner-profile-v5-{generation}-{index}")
}

fn hash(value: &str) -> String { format!("{:x}", Sha256::digest(value.as_bytes())) }

fn commit(store: &impl CredentialStore) -> Result<Option<Commit>, String> {
    let Some(value) = store.get(ACTIVE)? else { return Ok(None); };
    let result: Commit = serde_json::from_str(&value)
        .map_err(|_| "protected voice profile commit is corrupt".to_string())?;
    if result.version != 1 || result.hashes.is_empty() || result.hashes.len() > MAX_ENTRIES {
        return Err("protected voice profile commit is incompatible".into());
    }
    Ok(Some(result))
}

pub(crate) fn load(store: &impl CredentialStore) -> Result<Option<Vec<String>>, String> {
    let Some(active) = commit(store)? else { return Ok(None); };
    let values = active.hashes.iter().enumerate().map(|(index, expected)| {
        let value = store.get(&account(active.generation, index))?
            .ok_or_else(|| "protected voice profile entry is missing".to_string())?;
        if hash(&value) != *expected { return Err("protected voice profile entry is corrupt".into()); }
        Ok(value)
    }).collect::<Result<Vec<_>, String>>()?;
    Ok(Some(values))
}

pub(crate) fn save(store: &impl CredentialStore, values: &[String]) -> Result<(), String> {
    if values.is_empty() || values.len() > MAX_ENTRIES {
        return Err("protected voice profile entry count is invalid".into());
    }
    // A corrupt old commit can be replaced by explicit enrollment. It cannot
    // supply an identity or authorize cleanup of any guessed credential name.
    let previous = commit(store).ok().flatten();
    let generation = uuid::Uuid::new_v4();
    let next = Commit { version: 1, generation, hashes: values.iter().map(|value| hash(value)).collect() };
    let encoded_commit = serde_json::to_string(&next).map_err(|error| error.to_string())?;
    for (index, value) in values.iter().enumerate() {
        if let Err(error) = store.set(&account(generation, index), value) {
            for written in 0..=index { let _ = store.delete(&account(generation, written)); }
            return Err(error);
        }
    }
    if let Err(error) = store.set(ACTIVE, &encoded_commit) {
        // A backend may report an uncertain write. Read the commit before
        // removing staged entries; never dismantle a generation that became
        // active despite that error.
        if store.get(ACTIVE).ok().flatten().as_deref() != Some(encoded_commit.as_str()) {
            for index in 0..values.len() { let _ = store.delete(&account(generation, index)); }
            return Err(error);
        }
    }
    if let Some(previous) = previous {
        for index in 0..previous.hashes.len() { let _ = store.delete(&account(previous.generation, index)); }
    }
    Ok(())
}

pub(crate) fn delete(store: &impl CredentialStore) -> Result<(), String> {
    if let Some(active) = commit(store)? {
        for index in 0..active.hashes.len() { store.delete(&account(active.generation, index))?; }
    }
    store.delete(ACTIVE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::{Cell, RefCell}, collections::HashMap};

    #[derive(Default)]
    struct Store { values: RefCell<HashMap<String,String>>, writes: Cell<usize>, fail_at: Cell<Option<usize>> }
    impl CredentialStore for Store {
        fn get(&self, account: &str) -> Result<Option<String>,String> { Ok(self.values.borrow().get(account).cloned()) }
        fn set(&self, account: &str, value: &str) -> Result<(),String> {
            let write = self.writes.get(); self.writes.set(write + 1);
            if self.fail_at.get() == Some(write) { return Err("injected credential write failure".into()); }
            self.values.borrow_mut().insert(account.into(),value.into()); Ok(())
        }
        fn delete(&self, account: &str) -> Result<(),String> { self.values.borrow_mut().remove(account); Ok(()) }
    }

    #[test]
    fn windows_binary_credentials_preserve_unicode_and_the_exact_byte_limit() {
        let value = r#"{"phrase":"开始录音 日本語 한국어 🎙️","embedding":"abcdef"}"#;
        assert_eq!(decode_windows_credential(&encode_windows_credential(value).unwrap()).unwrap(), value);
        let legacy = value.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>();
        assert_eq!(decode_windows_credential(&legacy).unwrap(), value);
        let at_limit = "x".repeat(WINDOWS_CREDENTIAL_LIMIT_BYTES - WINDOWS_BINARY_PREFIX.len());
        assert_eq!(encode_windows_credential(&at_limit).unwrap().len(), WINDOWS_CREDENTIAL_LIMIT_BYTES);
        assert!(encode_windows_credential(&(at_limit + "x")).is_err());
        assert!(decode_windows_credential(b"\xff").is_err());
        let mut invalid = WINDOWS_BINARY_PREFIX.to_vec(); invalid.push(0xff);
        assert!(decode_windows_credential(&invalid).is_err());
    }

    #[test]
    fn failed_enrollment_preserves_every_old_entry_and_active_commit() {
        let old = vec!["old wake".into(),"old body".into()];
        let new = (0..8).map(|index| format!("new model {index}")).collect::<Vec<_>>();
        for failure in 0..=new.len() {
            let store = Store::default(); save(&store,&old).unwrap();
            let before = store.values.borrow().clone();
            store.fail_at.set(Some(store.writes.get() + failure));
            assert!(save(&store,&new).is_err());
            assert_eq!(load(&store).unwrap(),Some(old.clone()));
            assert_eq!(*store.values.borrow(),before);
        }
    }

    #[test]
    fn one_commit_switches_the_complete_generation_and_delete_removes_it() {
        let store = Store::default();
        save(&store,&["wake".into(),"old body".into()]).unwrap();
        save(&store,&["new wake".into(),"new body".into(),"new identity".into()]).unwrap();
        assert_eq!(load(&store).unwrap(),Some(vec!["new wake".into(),"new body".into(),"new identity".into()]));
        assert_eq!(store.values.borrow().len(),4);
        delete(&store).unwrap(); assert_eq!(load(&store).unwrap(),None);
        assert!(store.values.borrow().is_empty());
    }

    #[test]
    fn missing_or_changed_entry_cannot_become_a_partial_profile() {
        let store = Store::default(); save(&store,&["wake".into(),"identity".into()]).unwrap();
        let active = commit(&store).unwrap().unwrap();
        store.set(&account(active.generation,1),"wrong model").unwrap();
        assert!(load(&store).is_err());
        store.delete(&account(active.generation,1)).unwrap(); assert!(load(&store).is_err());
    }
}
