use crate::types::ShortcutBinding;

pub fn send_shortcut(binding: &ShortcutBinding) -> Result<(), String> {
    use enigo::{Direction, Enigo, Keyboard, Settings};

    let modifiers = shortcut_modifiers(binding)?;
    let primary = shortcut_primary(&binding.primary)?;
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    let mut pressed = 0usize;
    let mut first_err: Option<String> = None;

    for modifier in &modifiers {
        if let Err(e) = enigo.key(*modifier, Direction::Press) {
            first_err = Some(e.to_string());
            break;
        }
        pressed += 1;
    }

    if first_err.is_none() {
        if let Err(e) = enigo.key(primary, Direction::Click) {
            first_err = Some(e.to_string());
        }
    }

    for modifier in modifiers[..pressed].iter().rev() {
        if let Err(e) = enigo.key(*modifier, Direction::Release) {
            if first_err.is_none() {
                first_err = Some(e.to_string());
            }
        }
    }

    first_err.map_or(Ok(()), Err)
}

fn shortcut_modifiers(binding: &ShortcutBinding) -> Result<Vec<enigo::Key>, String> {
    let mut keys = Vec::new();
    for raw in &binding.modifiers {
        let key = match raw.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "super" | "meta" | "win" => enigo::Key::Meta,
            "ctrl" | "control" => enigo::Key::Control,
            "alt" | "option" | "opt" => enigo::Key::Alt,
            "shift" => enigo::Key::Shift,
            other => return Err(format!("unsupported shortcut modifier: {other}")),
        };
        keys.push(key);
    }
    Ok(keys)
}

fn shortcut_primary(raw: &str) -> Result<enigo::Key, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("shortcut primary is empty".into());
    }
    if trimmed.chars().count() == 1 {
        return Ok(enigo::Key::Unicode(
            trimmed.chars().next().expect("single char exists"),
        ));
    }

    let upper = trimmed.to_ascii_uppercase();
    let key = match upper.as_str() {
        "ENTER" | "RETURN" => enigo::Key::Return,
        "TAB" => enigo::Key::Tab,
        "ESC" | "ESCAPE" => enigo::Key::Escape,
        "SPACE" => enigo::Key::Space,
        "BACKSPACE" => enigo::Key::Backspace,
        "DELETE" | "DEL" => enigo::Key::Delete,
        "HOME" => enigo::Key::Home,
        "END" => enigo::Key::End,
        "PAGEUP" => enigo::Key::PageUp,
        "PAGEDOWN" => enigo::Key::PageDown,
        "ARROWUP" | "UP" => enigo::Key::UpArrow,
        "ARROWDOWN" | "DOWN" => enigo::Key::DownArrow,
        "ARROWLEFT" | "LEFT" => enigo::Key::LeftArrow,
        "ARROWRIGHT" | "RIGHT" => enigo::Key::RightArrow,
        "F1" => enigo::Key::F1,
        "F2" => enigo::Key::F2,
        "F3" => enigo::Key::F3,
        "F4" => enigo::Key::F4,
        "F5" => enigo::Key::F5,
        "F6" => enigo::Key::F6,
        "F7" => enigo::Key::F7,
        "F8" => enigo::Key::F8,
        "F9" => enigo::Key::F9,
        "F10" => enigo::Key::F10,
        "F11" => enigo::Key::F11,
        "F12" => enigo::Key::F12,
        "F13" => enigo::Key::F13,
        "F14" => enigo::Key::F14,
        "F15" => enigo::Key::F15,
        "F16" => enigo::Key::F16,
        _ => return Err(format!("unsupported shortcut primary: {trimmed}")),
    };
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_fallback_function_keys() {
        assert_eq!(
            shortcut_primary("F13").expect("F13 parses"),
            enigo::Key::F13
        );
        assert_eq!(
            shortcut_primary("F16").expect("F16 parses"),
            enigo::Key::F16
        );
    }

    #[test]
    fn parses_common_shortcut_modifiers() {
        let binding = ShortcutBinding {
            primary: "K".into(),
            modifiers: vec!["ctrl".into(), "shift".into()],
        };
        let modifiers = shortcut_modifiers(&binding).expect("modifiers parse");
        assert_eq!(modifiers, vec![enigo::Key::Control, enigo::Key::Shift]);
        assert_eq!(
            shortcut_primary(&binding.primary).expect("primary parses"),
            enigo::Key::Unicode('K')
        );
    }
}
