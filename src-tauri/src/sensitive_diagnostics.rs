use std::path::PathBuf;

/// Environment-driven speech captures are development tools, not a customer
/// recording preference. A release must ignore even a stale inherited variable.
pub(crate) fn development_enabled() -> bool {
    cfg!(any(debug_assertions, test))
}

fn directory_for_build(value: Option<String>, development: bool) -> Option<PathBuf> {
    if !development {
        return None;
    }
    value
        .filter(|directory| !directory.trim().is_empty())
        .map(PathBuf::from)
}

pub(crate) fn directory(value: Option<String>) -> Option<PathBuf> {
    directory_for_build(value, development_enabled())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn customer_build_ignores_an_inherited_capture_directory() {
        assert_eq!(directory_for_build(Some("private-audio".into()), false), None);
        assert_eq!(directory_for_build(None, true), None);
        assert_eq!(directory_for_build(Some(" ".into()), true), None);
        assert_eq!(directory_for_build(Some("private-audio".into()), true),
                   Some(PathBuf::from("private-audio")));
    }
}
