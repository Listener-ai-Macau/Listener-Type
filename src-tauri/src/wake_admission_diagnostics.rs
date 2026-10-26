//! Bounded process-local decisions, without audio, transcript or profile data.
//! Reading this receipt never starts capture or reads historical log files.
use parking_lot::Mutex;
use serde::Serialize;
use std::{collections::VecDeque, sync::OnceLock};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    at_ms: u64,
    candidate_id: u64,
    action: &'static str,
    stage: &'static str,
    intent: Option<String>,
    owner_access: Option<&'static str>,
    owner_score: Option<f32>,
    recovered_owner: bool,
    terminal: bool,
    outcome: String,
    elapsed_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    schema: &'static str,
    entries: Vec<Entry>,
}

fn ledger() -> &'static Mutex<VecDeque<Entry>> {
    static LEDGER: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn action_label(action: crate::voice_command::VoiceAction) -> &'static str {
    match action { crate::voice_command::VoiceAction::Dictation => "dictation",
        crate::voice_command::VoiceAction::Question => "question" }
}

fn retain(mut entry: Entry) {
    entry.at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default().as_millis().min(u64::MAX as u128) as u64;
    let mut entries = ledger().lock();
    if let Some(index) = entries.iter().position(|old|
        old.candidate_id == entry.candidate_id && old.action == entry.action && old.stage == entry.stage) {
        entries.remove(index);
    }
    while entries.len() >= 64 { entries.pop_front(); }
    entries.push_back(entry);
}

pub(crate) fn record_gate(candidate_id: u64, action: crate::voice_command::VoiceAction,
    intent: denzic_voice_activation_v1_core::PhraseSignal,
    gate: crate::speech_decision_kernel::WakeArbitration, score: Option<f32>, recovered_owner: bool,
    terminal: bool, elapsed_ms: u64) {
    retain(Entry { at_ms: 0, candidate_id, action: action_label(action), stage: "automatic_voice_gate",
        intent: Some(format!("{intent:?}")), owner_access: Some(gate.owner_access.policy_label()),
        owner_score: score.filter(|value| value.is_finite()), recovered_owner, terminal,
        outcome: format!("{:?}",gate.decision), elapsed_ms });
}

pub(crate) fn record_comparison(candidate_id: u64, action: crate::voice_command::VoiceAction,
    outcome: &'static str, elapsed_ms: u64) {
    retain(Entry { at_ms: 0, candidate_id, action: action_label(action), stage: "acoustic_comparison",
        intent: None, owner_access: None, owner_score: None, recovered_owner: false,
        terminal: false, outcome: outcome.into(), elapsed_ms });
}

pub fn snapshot() -> Snapshot {
    Snapshot { schema: "listener.wake_admission_metadata.v1", entries: ledger().lock().iter().cloned().collect() }
}
