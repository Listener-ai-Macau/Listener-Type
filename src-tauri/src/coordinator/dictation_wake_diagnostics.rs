// Wake diagnostic retention helpers.
// Included into `coordinator::dictation` via `include!`.

#[derive(Debug, Clone)]
struct WakeDiagnosticRetentionEntry {
    path: std::path::PathBuf,
    modified: std::time::SystemTime,
    bytes: u64,
}

fn wake_diagnostic_retention_plan(
    mut entries: Vec<WakeDiagnosticRetentionEntry>,
    now: std::time::SystemTime,
    max_age: Duration,
    max_files: usize,
    max_bytes: u64,
) -> Vec<std::path::PathBuf> {
    entries.sort_by_key(|entry| {
        entry
            .modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
    });

    let mut remove = Vec::new();
    let mut retained = Vec::with_capacity(entries.len());
    for entry in entries {
        let expired = now
            .duration_since(entry.modified)
            .is_ok_and(|age| age > max_age);
        if expired {
            remove.push(entry.path);
        } else {
            retained.push(entry);
        }
    }

    let mut retained_bytes = retained.iter().map(|entry| entry.bytes).sum::<u64>();
    let mut remove_count = 0usize;
    while retained.len().saturating_sub(remove_count) > max_files
        || retained_bytes > max_bytes
    {
        let Some(entry) = retained.get(remove_count) else {
            break;
        };
        retained_bytes = retained_bytes.saturating_sub(entry.bytes);
        remove.push(entry.path.clone());
        remove_count += 1;
    }
    remove
}
