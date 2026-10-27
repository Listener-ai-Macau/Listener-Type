//! Keep the DIS product model distinct from a comparable hardware revision.

pub(crate) fn normalize_listener_hardware_revision(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "keyboard-v2-n16r8" | "voice-keyboard-v2-n16r8" | "esp32s3-wroom-1-n16r8" => {
            Some("keyboard-v2-n16r8".to_string())
        }
        _ => None,
    }
}

pub(crate) fn normalize_listener_ota_hardware_revision(
    model: Option<String>,
    hardware: Option<String>,
) -> Option<String> {
    if let Some(hardware) = hardware.filter(|value| !value.trim().is_empty()) {
        // A reported incompatible revision must never be replaced by a model.
        return Some(normalize_listener_hardware_revision(&hardware).unwrap_or(hardware));
    }
    model
        .filter(|value| !value.trim().is_empty())
        .and_then(|model| {
            if model.trim().eq_ignore_ascii_case("keyboard-v2") {
                None
            } else {
                Some(normalize_listener_hardware_revision(&model).unwrap_or(model))
            }
        })
}

pub(crate) fn hardware_revision_requires_refresh(
    model: Option<&str>,
    revision: Option<&str>,
) -> bool {
    revision.is_none()
        && model.is_some_and(|value| value.trim().eq_ignore_ascii_case("keyboard-v2"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_dis_model_does_not_masquerade_as_a_different_revision() {
        let model = Some("keyboard-v2".to_string());
        let revision = normalize_listener_ota_hardware_revision(model.clone(), None);
        assert_eq!(revision, None);
        assert!(hardware_revision_requires_refresh(
            model.as_deref(),
            revision.as_deref()
        ));
    }

    #[test]
    fn physical_revision_has_priority_over_model_and_preserves_mismatches() {
        let model = Some("keyboard-v2".to_string());
        assert_eq!(
            normalize_listener_ota_hardware_revision(
                model.clone(),
                Some("esp32s3-wroom-1-n16r8".into())
            ),
            Some("keyboard-v2-n16r8".into())
        );
        assert_eq!(
            normalize_listener_ota_hardware_revision(model, Some("keyboard-v2-n8r2".into())),
            Some("keyboard-v2-n8r2".into())
        );
    }

    #[test]
    fn explicit_legacy_revision_and_unknown_metadata_keep_their_scopes() {
        assert_eq!(
            normalize_listener_ota_hardware_revision(Some("keyboard-v2-n16r8".into()), None),
            Some("keyboard-v2-n16r8".into())
        );
        assert_eq!(
            normalize_listener_ota_hardware_revision(Some("keyboard-v1".into()), None),
            Some("keyboard-v1".into())
        );
        assert!(!hardware_revision_requires_refresh(None, None));
    }
}
