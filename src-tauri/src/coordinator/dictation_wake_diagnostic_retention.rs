fn prune_default_wake_diagnostics(directory: &std::path::Path) -> Result<usize, String> {
    let mut entries = Vec::new();
    let read_dir =
        fs::read_dir(directory).map_err(|err| format!("read {}: {err}", directory.display()))?;
    for item in read_dir {
        let Ok(item) = item else {
            continue;
        };
        let path = item.path();
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let is_matching_wav = file_name.starts_with("wake-candidate-")
            && path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("wav"));
        if !is_matching_wav {
            continue;
        }
        let Ok(metadata) = item.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        entries.push(WakeDiagnosticRetentionEntry {
            path,
            modified: metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
            bytes: metadata.len(),
        });
    }

    let removals = wake_diagnostic_retention_plan(
        entries,
        std::time::SystemTime::now(),
        WAKE_DIAGNOSTIC_RETENTION_MAX_AGE,
        WAKE_DIAGNOSTIC_RETENTION_MAX_FILES,
        WAKE_DIAGNOSTIC_RETENTION_MAX_BYTES,
    );
    let mut removed = 0usize;
    for path in removals {
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::warn!(
                    "[wake-phrase] diagnostic retention could not remove {}: {}",
                    path.display(),
                    err
                );
            }
        }
    }
    Ok(removed)
}

