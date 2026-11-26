use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use libloading::Library;
use parking_lot::Mutex;

const MODEL_SHA256: &str = "65a3fb9f5ad7bfb96bf69e0dc4481df97f6ee60513c1d94ce981ba6effd524b1";
const MAX_CHARS: usize = 2_000;

#[repr(C)]
struct PunctuationModelConfig {
    ct_transformer: *const c_char,
    num_threads: i32,
    debug: i32,
    provider: *const c_char,
}

#[repr(C)]
struct PunctuationConfig {
    model: PunctuationModelConfig,
}

type Destroy = unsafe extern "C" fn(*const c_void);
type AddPunct = unsafe extern "C" fn(*const c_void, *const c_char) -> *const c_char;
type FreeText = unsafe extern "C" fn(*const c_char);

struct Punctuation {
    handle: NonNull<c_void>,
    destroy: Destroy,
    add_punct: AddPunct,
    free_text: FreeText,
    _library: Library,
}

// The handle is used only under the process-wide mutex. Moving it between
// workers does not change the CPU ONNX session or permit concurrent calls.
unsafe impl Send for Punctuation {}

impl Drop for Punctuation {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.handle.as_ptr()) };
    }
}

impl Punctuation {
    fn open(path: &Path) -> Result<Self> {
        let actual = crate::speech_vad_windows::sha256_file(path).map_err(|err| anyhow!(err))?;
        anyhow::ensure!(
            actual.eq_ignore_ascii_case(MODEL_SHA256),
            "local punctuation model hash mismatch"
        );
        crate::speech_vad_windows::pin_bundled_vad_onnx_runtime().map_err(|err| anyhow!(err))?;
        let runtime = crate::speech_vad_windows::bundled_vad_runtime_root()
            .context("bundled speech runtime is missing")?;
        let path = CString::new(path.to_string_lossy().as_bytes())?;
        let config = PunctuationConfig {
            model: PunctuationModelConfig {
                ct_transformer: path.as_ptr(),
                num_threads: 1,
                debug: 0,
                provider: c"cpu".as_ptr(),
            },
        };
        unsafe {
            let library = Library::new(runtime.join("sherpa-onnx-c-api.dll"))?;
            let create = library
                .get::<unsafe extern "C" fn(*const PunctuationConfig) -> *mut c_void>(
                    b"SherpaOnnxCreateOfflinePunctuation\0",
                )?;
            let destroy = *library.get(b"SherpaOnnxDestroyOfflinePunctuation\0")?;
            let add_punct = *library.get(b"SherpaOfflinePunctuationAddPunct\0")?;
            let free_text = *library.get(b"SherpaOfflinePunctuationFreeText\0")?;
            let handle =
                NonNull::new(create(&config)).context("create local punctuation session")?;
            Ok(Self {
                handle,
                destroy,
                add_punct,
                free_text,
                _library: library,
            })
        }
    }

    fn propose(&self, text: &str) -> Result<String> {
        let input = CString::new(text)?;
        unsafe {
            let ptr = (self.add_punct)(self.handle.as_ptr(), input.as_ptr());
            anyhow::ensure!(!ptr.is_null(), "local punctuation returned no result");
            let result = CStr::from_ptr(ptr).to_str().map(str::to_owned);
            (self.free_text)(ptr);
            Ok(result?)
        }
    }
}

fn is_chinese(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{20000}'..='\u{2fa1f}')
}

fn is_model_punctuation(c: char) -> bool {
    matches!(
        c,
        ',' | '.' | '?' | '!' | ';' | ':' | '，' | '。' | '？' | '！' | '；' | '：' | '、'
    )
}

fn is_existing_punctuation(c: char) -> bool {
    is_model_punctuation(c)
        || c.is_ascii_punctuation()
        || matches!(
            c,
            '（' | '）' | '「' | '」' | '“' | '”' | '‘' | '’' | '《' | '》' | '…' | '—'
        )
}

// Keep the original bytes and only fill punctuation gaps. The model may
// change casing or spaces, and may split an existing decimal or time.
fn merge_punctuation(original: &str, proposed: &str, preview: bool) -> String {
    let proposed_content: Vec<_> = proposed
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace() && !is_existing_punctuation(*c))
        .collect();
    let original_content: Vec<_> = original
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace() && !is_existing_punctuation(*c))
        .collect();
    if original_content.len() != proposed_content.len()
        || original_content
            .iter()
            .zip(&proposed_content)
            .any(|((_, left), (_, right))| !left.eq_ignore_ascii_case(right))
    {
        return original.to_owned();
    }

    let mut out = String::with_capacity(original.len());
    let mut offset = 0;
    for (index, (&(start, c), &(prediction_start, prediction))) in
        original_content.iter().zip(&proposed_content).enumerate()
    {
        let end = start + c.len_utf8();
        let prediction_end = prediction_start + prediction.len_utf8();
        let next = original_content.get(index + 1);
        let original_gap = &original[end..next.map_or(original.len(), |(pos, _)| *pos)];
        let predicted_gap = &proposed[prediction_end
            ..proposed_content
                .get(index + 1)
                .map_or(proposed.len(), |(pos, _)| *pos)];
        out.push_str(&original[offset..end]);
        offset = end;
        let inside_ascii = next.is_some_and(|(_, next)| {
            (c.is_ascii_alphanumeric() && next.is_ascii_alphanumeric() && original_gap.is_empty())
                || (c.is_ascii_digit() && next.is_ascii_digit())
        });
        if (next.is_some() || !preview)
            && !inside_ascii
            && !original_gap.contains(['\n', '\r'])
            && !original_gap.chars().any(is_existing_punctuation)
        {
            if let Some(mark) = predicted_gap.chars().find(|c| is_model_punctuation(*c)) {
                out.push(mark);
            }
        }
    }
    out.push_str(&original[offset..]);
    out
}

fn punctuation_model_path() -> Option<std::path::PathBuf> {
    crate::persistence::chinese_punctuation_model()
        .ok()
        .filter(|path| path.is_file())
}

static PUNCTUATION: Mutex<Option<Punctuation>> = Mutex::new(None);

pub(crate) async fn release_punctuation() {
    let _ = tokio::task::spawn_blocking(|| drop(PUNCTUATION.lock().take())).await;
}

pub(crate) async fn restore_chinese_punctuation(text: String, preview: bool) -> String {
    if !text.chars().any(is_chinese) || text.chars().take(MAX_CHARS + 1).count() > MAX_CHARS {
        return text;
    }
    let work = text.clone();
    match tokio::task::spawn_blocking(move || {
        let mut slot = PUNCTUATION.lock();
        if slot.is_none() {
            let path = punctuation_model_path()?;
            static LOAD_FAILED: OnceLock<()> = OnceLock::new();
            if LOAD_FAILED.get().is_some() {
                return None;
            }
            match Punctuation::open(&path) {
                Ok(model) => {
                    log::info!("[local-punctuation] verified Chinese model loaded");
                    *slot = Some(model);
                }
                Err(error) => {
                    let _ = LOAD_FAILED.set(());
                    log::warn!("[local-punctuation] unavailable: {error:#}");
                }
            }
        }
        let prediction = slot.as_ref()?.propose(&work).ok()?;
        Some(merge_punctuation(&work, &prediction, preview))
    })
    .await
    {
        Ok(Some(result)) => result,
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_punctuation_only_adds_marks_to_original_text() {
        let text = "报价12.5美元到货11:30，你看行吗";
        assert_eq!(
            merge_punctuation(text, "报价12. 5美元，到货11: 30，你看行吗？", false),
            "报价12.5美元，到货11:30，你看行吗？"
        );
        assert_eq!(
            merge_punctuation(text, "报价12.6美元到货11:30，你看行吗？", false),
            text
        );
        assert_eq!(
            merge_punctuation("我用Listener转写", "我用listener，转写。", false),
            "我用Listener，转写。"
        );
    }

    #[test]
    fn local_punctuation_preview_does_not_add_a_final_mark() {
        assert_eq!(
            merge_punctuation("你好你看行吗", "你好，你看行吗？", true),
            "你好，你看行吗"
        );
        assert_eq!(
            merge_punctuation("你看行吗？", "你看行吗。", true),
            "你看行吗？"
        );
    }

    #[test]
    #[ignore = "requires the verified optional CT-Transformer model in the local cache"]
    fn local_punctuation_runtime_restores_chinese() {
        let Some(model) = punctuation_model_path() else {
            return;
        };
        let punctuator = Punctuation::open(&model).unwrap();
        let raw = "你好现在有用吗就是在这个预览那些东西能出得来吗";
        let suggestion = punctuator.propose(raw).unwrap();
        let final_text = merge_punctuation(raw, &suggestion, false);
        assert!(final_text.ends_with('？'), "{final_text}");
        assert!(
            final_text.contains('，') || final_text.matches('？').count() == 2,
            "{final_text}"
        );
    }
}
