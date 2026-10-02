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
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTextPattern, UIA_EditControlTypeId,
    UIA_TextPatternId, UIA_WindowControlTypeId, UIA_CONTROLTYPE_ID,
    UIA_ValuePatternId, UIA_PATTERN_ID,
    TextPatternRangeEndpoint_Start, TextPatternRangeEndpoint_End,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use super::FocusedInputState;

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
    text_pattern: Option<bool>, value_pattern: Option<bool>) -> FocusedInputState {
    if control == UIA_EditControlTypeId {
        FocusedInputState::Editable
    } else if control == UIA_WindowControlTypeId && keyboard_focusable == Some(false)
        && text_pattern == Some(false) && value_pattern == Some(false) {
        FocusedInputState::NonEditable
    } else {
        FocusedInputState::Unknown
    }
}

pub(crate) struct WindowsSelectionReader {
    pattern: IUIAutomationTextPattern,
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
        Self::new_with_budget(target_hwnd, require_editable, false, &mut FocusedInputState::Unknown)
    }

    pub(crate) fn capture_for_paste() -> (Option<Self>, FocusedInputState) {
        let target = unsafe { GetForegroundWindow().0 as usize };
        let mut state = FocusedInputState::Unknown;
        let reader = Self::new_with_budget(target, true, true, &mut state).ok();
        (reader, state)
    }

    fn new_with_budget(target_hwnd: usize, require_editable: bool, bounded: bool,
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
        if require_editable {
            let control = unsafe { focused.CurrentControlType() }
                .map_err(|err| format!("focused editor control type unavailable: {err}"))?;
            if control == UIA_EditControlTypeId {
                *state = FocusedInputState::Editable;
            } else {
                // Positive container/capability facts, never a window title or
                // a failed text read. Provider failure stays Unknown; only a
                // successful native query can establish absent patterns.
                let focusable = unsafe { focused.CurrentIsKeyboardFocusable() }.ok().map(|v| v.as_bool());
                let text = pattern_available(&focused, UIA_TextPatternId);
                let value = pattern_available(&focused, UIA_ValuePatternId);
                if unsafe { GetForegroundWindow().0 as usize } == target_hwnd
                    && unsafe { focused.CurrentHasKeyboardFocus() }.ok().is_some_and(|v| v.as_bool()) {
                    *state = focused_input_state(control, focusable, text, value);
                }
                return Err("focused control is not an editable text field".into());
            }
        }
        let pattern: IUIAutomationTextPattern =
            unsafe { focused.GetCurrentPatternAs(UIA_TextPatternId) }
                .map_err(|err| format!("focused editor has no safe selection readback: {err}"))?;
        Ok(Self {
            _apartment: apartment,
            pattern,
        })
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
        let selected = unsafe { range.GetText(8_193) }.map_err(|err| err.to_string())?.to_string();
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
            let text = tail.GetText(8_193).map_err(|err| err.to_string())?.to_string();
            Ok(text.chars().all(|ch| matches!(ch, '\r' | '\n')))
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use windows::Win32::UI::Accessibility::{UIA_CustomControlTypeId, UIA_DocumentControlTypeId};

    #[test]
    fn exposed_window_container_is_distinct_from_unreadable_or_custom_editor() {
        assert_eq!(focused_input_state(UIA_WindowControlTypeId, Some(false), Some(false), Some(false)), FocusedInputState::NonEditable);
        assert_eq!(focused_input_state(UIA_EditControlTypeId, Some(true), Some(false), Some(false)), FocusedInputState::Editable);
        for control in [UIA_CustomControlTypeId, UIA_DocumentControlTypeId] {
            assert_eq!(focused_input_state(control, Some(false), Some(false), Some(false)), FocusedInputState::Unknown);
        }
        for facts in [(None, Some(false), Some(false)), (Some(false), None, Some(false)),
            (Some(false), Some(false), None), (Some(true), Some(false), Some(false)),
            (Some(false), Some(true), Some(false)), (Some(false), Some(false), Some(true))] {
            assert_eq!(focused_input_state(UIA_WindowControlTypeId, facts.0, facts.1, facts.2), FocusedInputState::Unknown);
        }
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
