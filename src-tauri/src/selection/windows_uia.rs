//! Read focused Windows text selections without synthesizing Ctrl+C.
//!
//! Ctrl+C interrupts terminal jobs, so accessibility failure must remain an
//! unavailable readback rather than falling back to a keyboard copy shortcut.

use std::time::Duration;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationValuePattern, UIA_EditControlTypeId, UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_RadioButtonControlTypeId, UIA_HyperlinkControlTypeId,
    UIA_TextPatternId, UIA_WindowControlTypeId, UIA_CONTROLTYPE_ID,
    UIA_ValuePatternId, UIA_PATTERN_ID, UIA_DocumentControlTypeId, UIA_IsReadOnlyAttributeId,
    TextPatternRangeEndpoint_Start, TextPatternRangeEndpoint_End,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use super::FocusedInputState;

fn native_boolean_attribute(value: &windows::core::VARIANT) -> Option<bool> {
    // VARIANT equality first checks the native type. Do not coerce numbers,
    // strings, MixedAttribute or NotSupported into permission to edit.
    if value == &windows::core::VARIANT::from(true) { Some(true) }
    else if value == &windows::core::VARIANT::from(false) { Some(false) }
    else { None }
}

fn text_pattern_read_only(pattern: &IUIAutomationTextPattern) -> Option<bool> {
    let range = unsafe { pattern.DocumentRange() }.ok()?;
    let value = unsafe { range.GetAttributeValue(UIA_IsReadOnlyAttributeId) }.ok()?;
    native_boolean_attribute(&value)
}

fn combined_read_only(value: Option<bool>, text: Option<bool>) -> Option<bool> {
    match (value, text) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), _) | (_, Some(false)) => Some(false),
        _ => None,
    }
}

fn document_receipt_allowed(allow: bool, control: UIA_CONTROLTYPE_ID,
    read_only: Option<bool>, enabled: Option<bool>, text_available: bool) -> bool {
    allow && control == UIA_DocumentControlTypeId && read_only == Some(false)
        && enabled == Some(true) && text_available
}

fn pattern_available(element: &IUIAutomationElement, pattern: UIA_PATTERN_ID) -> Option<bool> {
    use windows::core::{Interface, IUnknown};
    // Preserve the native S_OK/null distinction. The generated IUnknown
    // wrapper turns a successful null pattern into E_POINTER, hiding the
    // difference between an absent capability and a failed provider call.
    let mut raw = std::ptr::null_mut();
    let result = unsafe {
        (Interface::vtable(element).GetCurrentPattern)(Interface::as_raw(element), pattern, &mut raw)
    };
    let available = !raw.is_null();
    if available { unsafe { drop(IUnknown::from_raw(raw)); } }
    result.is_ok().then_some(available)
}

fn focused_input_state(control: UIA_CONTROLTYPE_ID, keyboard_focusable: Option<bool>,
    text_pattern: Option<bool>, value_pattern: Option<bool>, read_only: Option<bool>,
    enabled: Option<bool>) -> FocusedInputState {
    // Edit describes the control's role, not permission to change its value.
    // WinForms read-only transcript/instruction boxes expose Edit + TextPattern.
    // Only positive native facts may defer a paste; an unavailable provider
    // remains Unknown and keeps the existing paste fallback.
    if read_only == Some(true) || enabled == Some(false) {
        FocusedInputState::NonEditable
    } else if matches!(control, UIA_EditControlTypeId | UIA_DocumentControlTypeId)
        && read_only == Some(false) {
        FocusedInputState::Editable
    } else if text_pattern == Some(false) && value_pattern == Some(false)
        && (control == UIA_WindowControlTypeId && keyboard_focusable == Some(false)
            || matches!(control, UIA_ButtonControlTypeId | UIA_CheckBoxControlTypeId
                | UIA_RadioButtonControlTypeId | UIA_HyperlinkControlTypeId)) {
        // A confirmed button/link without text/value input capabilities is
        // not an opaque editor. Keyboard focus alone cannot authorize paste.
        FocusedInputState::NonEditable
    } else {
        FocusedInputState::Unknown
    }
}

pub(crate) struct WindowsSelectionReader {
    pattern: IUIAutomationTextPattern,
    focused: IUIAutomationElement,
    automation: IUIAutomation,
    target_hwnd: usize,
    paste_observation_only: bool,
    _apartment: ComApartment,
}

struct ComApartment(bool);

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

impl WindowsSelectionReader {
    pub(crate) fn new(target_hwnd: usize) -> Result<Self, String> {
        Self::new_with_control_policy(target_hwnd, true)
    }

    fn new_with_control_policy(target_hwnd: usize, require_editable: bool) -> Result<Self, String> {
        Self::new_with_budget(target_hwnd, require_editable, false, false, &mut FocusedInputState::Unknown)
    }

    pub(crate) fn capture_for_paste() -> (Option<Self>, FocusedInputState) {
        let target = unsafe { GetForegroundWindow().0 as usize };
        let mut state = FocusedInputState::Unknown;
        let reader = match Self::new_with_budget(target, true, true, true, &mut state) {
            Ok(reader) => Some(reader),
            Err(error) => {
                // Capability failures have different causes (native provider,
                // focus, readonly target). Record the reason without editor text.
                log::info!("[insertion] native paste capture unavailable focus={state:?} reason={error}");
                None
            }
        };
        (reader, state)
    }

    fn new_with_budget(target_hwnd: usize, require_editable: bool, bounded: bool,
        allow_document_receipt: bool,
        state: &mut FocusedInputState) -> Result<Self, String> {
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let apartment = if init.is_ok() {
            ComApartment(true)
        } else if init == RPC_E_CHANGED_MODE {
            ComApartment(false)
        } else {
            return Err(format!("accessibility COM initialization failed: {init}"));
        };
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(if bounded { &CUIAutomation8 } else { &CUIAutomation }, None, CLSCTX_INPROC_SERVER) }
                .map_err(|err| format!("accessibility service unavailable: {err}"))?;
        if bounded {
            use windows::core::Interface;
            let timed = automation.cast::<IUIAutomation2>()
                .map_err(|err| format!("bounded accessibility service unavailable: {err}"))?;
            unsafe {
                timed.SetConnectionTimeout(50).map_err(|err| err.to_string())?;
                timed.SetTransactionTimeout(50).map_err(|err| err.to_string())?;
            }
        }
        let focused = unsafe { automation.GetFocusedElement() }
            .map_err(|err| format!("focused editor unavailable: {err}"))?;
        if unsafe { GetForegroundWindow().0 as usize } != target_hwnd {
            return Err("session target changed during accessibility lookup".into());
        }
        if !unsafe { focused.CurrentHasKeyboardFocus() }
            .map_err(|err| format!("focused editor state unavailable: {err}"))?
            .as_bool()
        {
            return Err("accessibility editor does not have keyboard focus".into());
        }
        let mut document_pattern: Option<IUIAutomationTextPattern> = None;
        let mut paste_observation_only = false;
        if require_editable {
            let control = unsafe { focused.CurrentControlType() }
                .map_err(|err| format!("focused editor control type unavailable: {err}"))?;
            // Read-only/disabled are mutation facts for every focused control,
            // including Chromium's Document role. They cannot become Unknown
            // just because its role differs from Edit; that would dispatch a
            // paste with no input target and reserve a false delivered prefix.
            let value: Option<IUIAutomationValuePattern> =
                unsafe { focused.GetCurrentPatternAs(UIA_ValuePatternId) }.ok();
            let value_read_only = value.as_ref()
                .and_then(|value| unsafe { value.CurrentIsReadOnly() }.ok())
                .map(|value| value.as_bool());
            // Contenteditable Chromium documents may expose TextPattern and
            // IsReadOnly=false while legitimately having no ValuePattern.
            // Read this native fact; neither a role nor pattern presence is
            // evidence that an unavailable readonly attribute means false.
            let read_only = if control == UIA_DocumentControlTypeId {
                document_pattern = unsafe { focused.GetCurrentPatternAs(UIA_TextPatternId) }.ok();
                combined_read_only(value_read_only, document_pattern.as_ref().and_then(text_pattern_read_only))
            } else { value_read_only };
            let enabled = unsafe { focused.CurrentIsEnabled() }.ok().map(|value| value.as_bool());
            if unsafe { GetForegroundWindow().0 as usize } != target_hwnd
                || !unsafe { focused.CurrentHasKeyboardFocus() }.ok().is_some_and(|v| v.as_bool()) {
                return Err("session target changed during editability lookup".into());
            }
            *state = focused_input_state(control, None, None, None, read_only, enabled);
            if *state == FocusedInputState::NonEditable {
                return Err("focused control is read-only or disabled".into());
            }
            if document_receipt_allowed(allow_document_receipt, control, read_only,
                enabled, document_pattern.is_some()) {
                paste_observation_only = true;
            } else if control != UIA_EditControlTypeId {
                // Positive container/capability facts, never a window title or
                // a failed text read. Missing mutation permission stays Unknown.
                let focusable = unsafe { focused.CurrentIsKeyboardFocusable() }.ok().map(|v| v.as_bool());
                let text = pattern_available(&focused, UIA_TextPatternId);
                let value = pattern_available(&focused, UIA_ValuePatternId);
                if unsafe { GetForegroundWindow().0 as usize } == target_hwnd
                    && unsafe { focused.CurrentHasKeyboardFocus() }.ok().is_some_and(|v| v.as_bool()) {
                    *state = focused_input_state(control, focusable, text, value, read_only, enabled);
                }
                return Err(format!("focused control is not an editable text field: role={} readonly={read_only:?} enabled={enabled:?} text={text:?} value={value:?}", control.0));
            }
        }
        let pattern: IUIAutomationTextPattern = match document_pattern {
            Some(pattern) => pattern,
            None => unsafe { focused.GetCurrentPatternAs(UIA_TextPatternId) }
                .map_err(|err| format!("focused editor has no safe selection readback: {err}"))?,
        };
        Ok(Self {
            _apartment: apartment,
            pattern,
            focused,
            automation,
            target_hwnd,
            paste_observation_only,
        })
    }

    /// Reuse only this thread's bounded native interface, never its old text.
    /// Each duplicate owns a balanced COM apartment reference of its own.
    pub(crate) fn duplicate_current_for_paste(&self) -> Result<Self, String> {
        if !self.current_observation()? { return Err("paste editor identity changed".into()); }
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let apartment = if init.is_ok() { ComApartment(true) }
            else if init == RPC_E_CHANGED_MODE { ComApartment(false) }
            else { return Err(format!("accessibility COM duplication failed: {init}")); };
        Ok(Self { pattern: self.pattern.clone(), focused: self.focused.clone(),
            automation: self.automation.clone(), target_hwnd: self.target_hwnd,
            paste_observation_only: self.paste_observation_only, _apartment: apartment })
    }

    pub(crate) fn read_selected_text(&self) -> Result<String, String> {
        for _ in 0..8 {
            let ranges = unsafe { self.pattern.GetSelection() }
                .map_err(|err| format!("accessibility selection unavailable: {err}"))?;
            if unsafe { ranges.Length() }
                .map_err(|err| format!("accessibility selection length unavailable: {err}"))?
                != 1
            {
                return Err("accessibility selection is not one contiguous range".into());
            }
            let range = unsafe { ranges.GetElement(0) }
                .map_err(|err| format!("accessibility selection range unavailable: {err}"))?;
            let value = unsafe { range.GetText(8_193) }
                .map_err(|err| format!("accessibility selected text unavailable: {err}"))?
                .to_string();
            if !value.is_empty() {
                return Ok(value);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err("accessibility selection did not expose text".into())
    }

    pub(crate) fn read_document_text(&self) -> Result<String, String> {
        let range = unsafe { self.pattern.DocumentRange() }
            .map_err(|err| format!("accessibility editor text unavailable: {err}"))?;
        // Some native empty Edit ranges return the accessible label from
        // GetText. Endpoint equality describes an empty range, not label text.
        if unsafe { range.CompareEndpoints(TextPatternRangeEndpoint_Start, &range, TextPatternRangeEndpoint_End) }
            .map_err(|err| err.to_string())? == 0 { return Ok(String::new()); }
        unsafe { range.GetText(8_193) }
            .map(|text| text.to_string())
            .map_err(|err| format!("accessibility editor text read failed: {err}"))
    }

    pub(crate) fn paste_snapshot(&self) -> Result<(String, String), String> {
        let document = self.bounded_document()?;
        let ranges = unsafe { self.pattern.GetSelection() }.map_err(|err| err.to_string())?;
        if unsafe { ranges.Length() }.map_err(|err| err.to_string())? != 1 {
            return Err("paste selection is not one contiguous range".into());
        }
        let range = unsafe { ranges.GetElement(0) }.map_err(|err| err.to_string())?;
        // Empty means an ordinary caret, not an unavailable selection.
        let selected = if unsafe { range.CompareEndpoints(TextPatternRangeEndpoint_Start, &range, TextPatternRangeEndpoint_End) }
            .map_err(|err| err.to_string())? == 0 { String::new() }
            else { unsafe { range.GetText(8_193) }.map_err(|err| err.to_string())?.to_string() };
        if selected.chars().count() > 8_192 {
            return Err("paste selection exceeds readback limit".into());
        }
        Ok((document, selected))
    }

    pub(crate) fn bounded_document(&self) -> Result<String, String> {
        let value = self.read_document_text()?;
        if value.chars().count() > 8_192 {
            return Err("paste document exceeds readback limit".into());
        }
        Ok(value)
    }

    pub(crate) fn revisable_document_agrees(&self, document: &str) -> Result<bool, String> {
        if self.paste_observation_only { return Ok(false); }
        let value: IUIAutomationValuePattern = unsafe { self.focused.GetCurrentPatternAs(UIA_ValuePatternId) }
            .map_err(|error| error.to_string())?;
        let value = unsafe { value.CurrentValue() }.map_err(|error| error.to_string())?.to_string();
        Ok(native_text_matches_value(document, &value)
            && self.bounded_document()? == document && self.current_observation()?)
    }

    pub(crate) fn expected_document_after_paste(&self, before: &(String, String), payload: &str) -> Result<String, String> {
        if self.paste_snapshot()? != *before { return Err("paste selection changed".into()); }
        unsafe {
            let selections = self.pattern.GetSelection().map_err(|e| e.to_string())?;
            if selections.Length().map_err(|e| e.to_string())? != 1 {
                return Err("paste selection is not contiguous".into());
            }
            let selection = selections.GetElement(0).map_err(|e| e.to_string())?;
            let document = self.pattern.DocumentRange().map_err(|e| e.to_string())?;
            let prefix = document.Clone().map_err(|e| e.to_string())?;
            prefix.MoveEndpointByRange(TextPatternRangeEndpoint_End, &selection, TextPatternRangeEndpoint_Start)
                .map_err(|e| e.to_string())?;
            let suffix = document.Clone().map_err(|e| e.to_string())?;
            suffix.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &selection, TextPatternRangeEndpoint_End)
                .map_err(|e| e.to_string())?;
            let read = |range: &windows::Win32::UI::Accessibility::IUIAutomationTextRange| -> Result<String, String> {
                if range.CompareEndpoints(TextPatternRangeEndpoint_Start, range, TextPatternRangeEndpoint_End)
                    .map_err(|e| e.to_string())? == 0 { return Ok(String::new()); }
                range.GetText(8_193).map(|s| s.to_string()).map_err(|e| e.to_string())
            };
            let expected = super::native_edit_result(&before.0, &before.1, &read(&prefix)?, &read(&suffix)?, payload)?;
            if self.paste_snapshot()? != *before { return Err("native document changed during edit prediction".into()); }
            Ok(expected)
        }
    }

    pub(crate) fn same_target(&self, other: &Self) -> bool {
        self.same_target_observation(other).unwrap_or(false)
    }

    pub(crate) fn same_target_observation(&self, other: &Self) -> Result<bool, String> {
        if self.target_hwnd != other.target_hwnd { return Ok(false); }
        unsafe { self.automation.CompareElements(&self.focused, &other.focused) }
            .map(|same| same.as_bool()).map_err(|error| error.to_string())
    }

    pub(crate) fn is_current(&self) -> bool {
        self.current_observation().unwrap_or(false)
    }

    pub(crate) fn current_observation(&self) -> Result<bool, String> {
        unsafe {
            if GetForegroundWindow().0 as usize != self.target_hwnd { return Ok(false); }
            let focused = self.automation.GetFocusedElement().map_err(|error| error.to_string())?;
            if !self.automation.CompareElements(&self.focused, &focused).map_err(|error| error.to_string())?.as_bool()
                || !self.focused.CurrentHasKeyboardFocus().map_err(|error| error.to_string())?.as_bool()
                || !self.focused.CurrentIsEnabled().map_err(|error| error.to_string())?.as_bool() {
                return Ok(false);
            }
            if self.paste_observation_only {
                if self.focused.CurrentControlType().map_err(|error| error.to_string())? != UIA_DocumentControlTypeId {
                    return Ok(false);
                }
                let value: Option<IUIAutomationValuePattern> = self.focused.GetCurrentPatternAs(UIA_ValuePatternId).ok();
                let value_read_only = value.as_ref().and_then(|v| v.CurrentIsReadOnly().ok()).map(|v| v.as_bool());
                return Ok(combined_read_only(value_read_only, text_pattern_read_only(&self.pattern)) == Some(false));
            }
            let value: IUIAutomationValuePattern = self.focused.GetCurrentPatternAs(UIA_ValuePatternId)
                .map_err(|error| error.to_string())?;
            Ok(!value.CurrentIsReadOnly().map_err(|error| error.to_string())?.as_bool())
        }
    }

    /// Select only the receipted session suffix; never synthesize Ctrl+A or
    /// move focus. UIA's character units must expose the exact expected bytes.
    pub(crate) fn select_owned_suffix(&self, document: &str, expected: &str) -> Result<(), String> {
        if self.paste_observation_only { return Err("paste observation does not grant native selection mutation".into()); }
        if expected.is_empty() || !self.is_current() || self.paste_snapshot()? != (document.into(), String::new())
            || !self.caret_at_end()? { return Err("session editor changed".into()); }
        unsafe {
            let selection = self.pattern.GetSelection().map_err(|e| e.to_string())?
                .GetElement(0).map_err(|e| e.to_string())?;
            let range = selection.Clone().map_err(|e| e.to_string())?;
            range.MoveEndpointByUnit(TextPatternRangeEndpoint_Start,
                windows::Win32::UI::Accessibility::TextUnit_Character,
                -(expected.chars().count() as i32)).map_err(|e| e.to_string())?;
            if range.GetText(8_193).map_err(|e| e.to_string())?.to_string() != expected
                || !self.is_current() || self.bounded_document()? != document {
                return Err("session suffix is not an exact native range".into());
            }
            range.Select().map_err(|e| e.to_string())?;
        }
        if !self.is_current() || self.paste_snapshot()? != (document.into(), expected.into()) {
            return Err("session selection changed".into());
        }
        Ok(())
    }

    pub(crate) fn caret_at_end(&self) -> Result<bool, String> {
        unsafe {
            let ranges = self.pattern.GetSelection().map_err(|err| err.to_string())?;
            if ranges.Length().map_err(|err| err.to_string())? != 1 {
                return Ok(false);
            }
            let selection = ranges.GetElement(0).map_err(|err| err.to_string())?;
            if selection.CompareEndpoints(TextPatternRangeEndpoint_Start, &selection, TextPatternRangeEndpoint_End)
                .map_err(|err| err.to_string())? != 0 {
                return Ok(false);
            }
            let tail = self.pattern.DocumentRange().map_err(|err| err.to_string())?;
            tail.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &selection, TextPatternRangeEndpoint_End)
                .map_err(|err| err.to_string())?;
            if tail.CompareEndpoints(TextPatternRangeEndpoint_Start, &tail, TextPatternRangeEndpoint_End)
                .map_err(|err| err.to_string())? == 0 { return Ok(true); }
            let text = tail.GetText(8_193).map_err(|err| err.to_string())?.to_string();
            Ok(text.chars().all(|ch| matches!(ch, '\r' | '\n')))
        }
    }
}

fn native_text_matches_value(document: &str, value: &str) -> bool {
    // UIA may expose a terminal paragraph marker. Preserve the complete value
    // and permit only that suffix; a placeholder is never a user-text prefix.
    document.strip_prefix(value).is_some_and(|suffix|
        suffix.chars().all(|ch| matches!(ch, '\r' | '\n')))
}

#[cfg(test)]
mod focus_tests {
    use super::*;

    #[test]
    fn saved_writable_document_supports_paste_receipts_without_value_pattern() {
        // 14:53 failed paste: Document=50030, enabled and TextPattern present,
        // ValuePattern absent. At 14:55 the same native provider exposed a
        // Boolean TextPattern.IsReadOnly=false. No title/text heuristic is used.
        let read_only = combined_read_only(None, native_boolean_attribute(&windows::core::VARIANT::from(false)));
        assert_eq!(focused_input_state(UIA_DocumentControlTypeId, Some(true),
            Some(true), Some(false), read_only, Some(true)), FocusedInputState::Editable);
        assert!(document_receipt_allowed(true, UIA_DocumentControlTypeId,
            read_only, Some(true), true));
        assert!(!document_receipt_allowed(false, UIA_DocumentControlTypeId,
            read_only, Some(true), true), "ordinary selection/range mutation does not gain this capability");
        for (readonly, enabled, text) in [(None, Some(true), true),
            (Some(true), Some(true), true), (Some(false), None, true),
            (Some(false), Some(false), true), (Some(false), Some(true), false)] {
            assert!(!document_receipt_allowed(true, UIA_DocumentControlTypeId, readonly, enabled, text));
        }
        assert_eq!(combined_read_only(Some(true), Some(false)), Some(true));
        assert_eq!(combined_read_only(Some(false), Some(true)), Some(true));
        assert_eq!(focused_input_state(UIA_DocumentControlTypeId, Some(true),
            Some(true), Some(false), Some(true), Some(true)), FocusedInputState::NonEditable,
            "the saved readonly transcript must remain excluded");
    }

    #[test]
    fn native_readonly_attribute_requires_a_boolean_not_coercible_or_reserved_values() {
        use windows::core::VARIANT;
        assert_eq!(native_boolean_attribute(&VARIANT::from(true)), Some(true));
        assert_eq!(native_boolean_attribute(&VARIANT::from(false)), Some(false));
        for value in [VARIANT::new(), VARIANT::from(0i32), VARIANT::from(-1i32),
            VARIANT::from(0.0f64), VARIANT::from("false"), VARIANT::from("true")] {
            assert_eq!(native_boolean_attribute(&value), None,
                "numeric/string/default attributes cannot grant paste permission");
        }
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        assert!(init.is_ok() || init == RPC_E_CHANGED_MODE);
        let _apartment = ComApartment(init.is_ok());
        use windows::Win32::UI::Accessibility::{UiaGetReservedMixedAttributeValue,
            UiaGetReservedNotSupportedValue};
        for reserved in [unsafe { UiaGetReservedMixedAttributeValue() }.unwrap(),
            unsafe { UiaGetReservedNotSupportedValue() }.unwrap()] {
            assert_eq!(native_boolean_attribute(&VARIANT::from(reserved)), None,
                "native UIA MixedAttribute/NotSupported cannot grant paste permission");
        }
    }

    #[test]
    fn readonly_documents_and_nontext_buttons_cannot_be_unknown_paste_targets() {
        // Saved Codex native observations: Document+readonly and focused
        // Button (50000)+no Text/Value patterns. Neither is an input target.
        assert_eq!(focused_input_state(UIA_DocumentControlTypeId, Some(true),
            Some(true), Some(true), Some(true), Some(true)), FocusedInputState::NonEditable);
        for role in [UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId,
            UIA_RadioButtonControlTypeId, UIA_HyperlinkControlTypeId] {
            assert_eq!(focused_input_state(role, Some(true), Some(false),
                Some(false), None, Some(true)), FocusedInputState::NonEditable);
            assert_eq!(focused_input_state(role, Some(true), None,
                Some(false), None, Some(true)), FocusedInputState::Unknown,
                "a provider failure is not positive evidence that TextPattern is absent");
        }
        assert_eq!(focused_input_state(UIA_CustomControlTypeId, Some(true),
            Some(false), Some(false), None, Some(true)), FocusedInputState::Unknown);
    }

    #[test]
    fn reversible_baseline_requires_real_value_and_preserves_user_text() {
        assert!(!native_text_matches_value("输入内容\n", ""), "an empty editor's prompt cannot be a user prefix");
        assert!(!native_text_matches_value("用户原文\n", "新原文"));
        for text in ["", "用户原文🔒：\r\n", "日本語\n한국어", "Words at 9."] {
            assert!(native_text_matches_value(text, text));
            assert!(native_text_matches_value(&format!("{text}\r\n"), text));
            assert!(!native_text_matches_value(&format!("{text} extra"), text));
        }
    }
    use windows::Win32::UI::Accessibility::UIA_CustomControlTypeId;

    #[test]
    fn exposed_window_container_is_distinct_from_unreadable_or_custom_editor() {
        assert_eq!(focused_input_state(UIA_WindowControlTypeId, Some(false), Some(false), Some(false), None, None), FocusedInputState::NonEditable);
        assert_eq!(focused_input_state(UIA_EditControlTypeId, Some(true), Some(false), Some(false), Some(false), Some(true)), FocusedInputState::Editable);
        for control in [UIA_CustomControlTypeId, UIA_DocumentControlTypeId] {
            assert_eq!(focused_input_state(control, Some(false), Some(false), Some(false), None, None), FocusedInputState::Unknown);
        }
        for facts in [(None, Some(false), Some(false)), (Some(false), None, Some(false)),
            (Some(false), Some(false), None), (Some(true), Some(false), Some(false)),
            (Some(false), Some(true), Some(false)), (Some(false), Some(false), Some(true))] {
            assert_eq!(focused_input_state(UIA_WindowControlTypeId, facts.0, facts.1, facts.2, None, None), FocusedInputState::Unknown);
        }
    }

    #[test]
    fn read_only_edit_role_cannot_consume_dictation_but_unknown_accessibility_can_fallback() {
        // The failed long recording moved from an editable note to a read-only
        // 472-character instruction box. Both expose Edit and TextPattern.
        for enabled in [None, Some(true), Some(false)] {
            assert_eq!(focused_input_state(UIA_EditControlTypeId, Some(true), Some(true), Some(true), Some(true), enabled), FocusedInputState::NonEditable);
        }
        assert_eq!(focused_input_state(UIA_EditControlTypeId, Some(true), Some(true), Some(true), Some(false), Some(false)), FocusedInputState::NonEditable);
        assert_eq!(focused_input_state(UIA_EditControlTypeId, Some(true), Some(true), Some(true), Some(false), Some(true)), FocusedInputState::Editable);
        assert_eq!(focused_input_state(UIA_EditControlTypeId, Some(true), Some(true), None, None, Some(true)), FocusedInputState::Unknown);
    }
}

pub(crate) fn read_focused_selection() -> Option<String> {
    let target = unsafe { GetForegroundWindow().0 as usize };
    if target == 0 {
        return None;
    }
    // QA only reads an existing selection. Web documents are allowed here;
    // final text replacement uses `new`, which requires an editable control.
    WindowsSelectionReader::new_with_control_policy(target, false)
        .and_then(|reader| reader.read_selected_text())
        .ok()
}
