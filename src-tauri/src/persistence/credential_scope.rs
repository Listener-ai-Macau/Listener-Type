//! Opt-in credential scope for noninteractive private data profiles.
//! No OS keyring access or fallback lives here; invalid scopes fail closed.
use std::path::Path;

pub(crate) fn service_name(
    default_service: &str,
    namespace: Option<&std::ffi::OsStr>,
    profile_dir: Option<&std::ffi::OsStr>,
) -> Result<String, &'static str> {
    let Some(namespace) = namespace else {
        // Existing daily and portable instances retain their original vault.
        return Ok(default_service.to_string());
    };
    let namespace = namespace
        .to_str()
        .ok_or("invalid private credential namespace")?;
    if namespace.is_empty()
        || namespace.len() > 64
        || !namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("invalid private credential namespace");
    }
    if !profile_dir.is_some_and(|path| Path::new(path).is_absolute()) {
        return Err("private credentials require an absolute data directory");
    }
    Ok(format!("{default_service}.profile.{namespace}"))
}

#[cfg(test)]
mod tests {
    use super::service_name;
    const DEFAULT_SERVICE: &str = "com.listener.type";

    #[test]
    fn credential_scope_preserves_daily_default_and_separates_private_profiles() {
        use std::ffi::OsStr;
        let root = std::env::temp_dir().join("listener-private-credential-scope");
        assert!(root.is_absolute());
        assert_eq!(
            service_name(DEFAULT_SERVICE, None, None).unwrap(),
            "com.listener.type"
        );
        assert_eq!(
            service_name(DEFAULT_SERVICE, None, Some(root.as_os_str())).unwrap(),
            "com.listener.type"
        );
        let first = service_name(
            DEFAULT_SERVICE,
            Some(OsStr::new("first_88")),
            Some(root.as_os_str()),
        )
        .unwrap();
        let second = service_name(
            DEFAULT_SERVICE,
            Some(OsStr::new("second-88")),
            Some(root.as_os_str()),
        )
        .unwrap();
        assert_eq!(first, "com.listener.type.profile.first_88");
        assert_ne!(first, second);
        assert_ne!(first, DEFAULT_SERVICE);
    }

    #[test]
    fn credential_scope_rejects_invalid_profiles_without_falling_back_to_daily() {
        use std::ffi::OsStr;
        let root = std::env::temp_dir().join("listener-private-credential-scope");
        for invalid in ["", " ", "daily.profile", "../daily", "a/b", "a\\b", "中文"] {
            assert!(service_name(
                DEFAULT_SERVICE,
                Some(OsStr::new(invalid)),
                Some(root.as_os_str())
            )
            .is_err());
        }
        let too_long = "a".repeat(65);
        assert!(service_name(
            DEFAULT_SERVICE,
            Some(OsStr::new(&too_long)),
            Some(root.as_os_str())
        )
        .is_err());
        for invalid_root in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("relative/profile")),
        ] {
            assert!(
                service_name(DEFAULT_SERVICE, Some(OsStr::new("private88")), invalid_root).is_err()
            );
        }
    }
}
