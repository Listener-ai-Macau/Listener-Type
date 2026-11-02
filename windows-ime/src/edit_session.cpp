#include "edit_session.h"
#include "context_document_guard.h"
#include "focus_context_guard.h"
#include "terminal_tail_guard.h"

#include <utility>

extern LONG g_object_count;

ListenerTypeAsyncEditState::ListenerTypeAsyncEditState()
    : event(CreateEventW(nullptr, TRUE, FALSE, nullptr)) {
  if (event == nullptr) {
    create_error = GetLastError();
  }
}

ListenerTypeAsyncEditState::~ListenerTypeAsyncEditState() {
  if (event != nullptr) {
    CloseHandle(event);
    event = nullptr;
  }
}

bool ListenerTypeAsyncEditState::IsValid() const {
  return event != nullptr;
}

ListenerTypeEditSession::ListenerTypeEditSession(
    ITfContext* context,
    std::wstring text,
    ListenerTypeCompositionOp op,
    ITfComposition** composition_slot,
    std::shared_ptr<ListenerTypeAsyncEditState> async_state,
    ITfThreadMgr* focus_guard,
    std::wstring expected_body,
    HWND expected_foreground,
    std::shared_ptr<ListenerTypeStopSignal> stop_signal,
    std::optional<std::wstring> expected_document)
    : context_(context),
      text_(std::move(text)),
      op_(op),
      composition_slot_(composition_slot),
      async_state_(std::move(async_state)),
      focus_guard_(focus_guard),
      expected_body_(std::move(expected_body)),
      expected_foreground_(expected_foreground),
      stop_signal_(std::move(stop_signal)),
      expected_document_(std::move(expected_document)) {
  // TSF may hold an asynchronous edit after the text service deactivates.
  // Keep DllCanUnloadNow false until the edit session's last reference ends.
  InterlockedIncrement(&g_object_count);
  if (context_ != nullptr) {
    context_->AddRef();
  }
  if (focus_guard_ != nullptr) focus_guard_->AddRef();
}

ListenerTypeEditSession::~ListenerTypeEditSession() {
  if (focus_guard_ != nullptr) focus_guard_->Release();
  if (context_ != nullptr) {
    context_->Release();
    context_ = nullptr;
  }
  InterlockedDecrement(&g_object_count);
}

STDMETHODIMP ListenerTypeEditSession::QueryInterface(REFIID iid, void** object) {
  if (object == nullptr) {
    return E_POINTER;
  }
  *object = nullptr;

  if (iid == IID_IUnknown || iid == IID_ITfEditSession) {
    *object = static_cast<ITfEditSession*>(this);
    AddRef();
    return S_OK;
  }

  return E_NOINTERFACE;
}

STDMETHODIMP_(ULONG) ListenerTypeEditSession::AddRef() {
  return static_cast<ULONG>(InterlockedIncrement(&ref_count_));
}

STDMETHODIMP_(ULONG) ListenerTypeEditSession::Release() {
  const ULONG count = static_cast<ULONG>(InterlockedDecrement(&ref_count_));
  if (count == 0) {
    delete this;
  }
  return count;
}

STDMETHODIMP ListenerTypeEditSession::DoEditSession(TfEditCookie edit_cookie) {
  HRESULT hr;
  if (StopRequestedBeforeWrite()) {
    hr = HRESULT_FROM_WIN32(ERROR_CANCELLED);
    if (async_state_) {
      async_state_->result = hr;
      if (async_state_->event != nullptr) SetEvent(async_state_->event);
    }
    return hr;
  }
  switch (op_) {
    case ListenerTypeCompositionOp::kInsertOnce:
      hr = InsertText(edit_cookie);
      break;
    case ListenerTypeCompositionOp::kInsertGuarded:
      // Async edit callbacks may run after the user has changed controls.
      if ((expected_foreground_ != nullptr && GetForegroundWindow() != expected_foreground_) ||
          !ListenerFocusedContextIs(focus_guard_, context_) ||
          (expected_document_ && !ListenerContextDocumentEquals(context_, edit_cookie, *expected_document_))) {
        MarkWritePrevented();
        hr = HRESULT_FROM_WIN32(ERROR_CANCELLED);
      } else {
        hr = InsertText(edit_cookie);
      }
      break;
    case ListenerTypeCompositionOp::kInsertGuardedTerminal: {
      if ((expected_foreground_ != nullptr && GetForegroundWindow() != expected_foreground_) ||
          !ListenerFocusedContextIs(focus_guard_, context_) ||
          (expected_document_ && !ListenerContextDocumentEquals(context_, edit_cookie, *expected_document_))) {
        MarkWritePrevented();
        hr = HRESULT_FROM_WIN32(ERROR_CANCELLED);
        break;
      }
      const auto already = ListenerInspectTerminalTail(context_, edit_cookie, expected_body_ + text_);
      if (already == ListenerTerminalTail::kMatches) { hr = S_FALSE; break; }
      if (already == ListenerTerminalTail::kUnavailable) { MarkWritePrevented(); hr = E_FAIL; break; }
      const auto before = ListenerInspectTerminalTail(context_, edit_cookie, expected_body_);
      if (before != ListenerTerminalTail::kMatches || StopRequestedBeforeWrite() ||
          (expected_foreground_ != nullptr && GetForegroundWindow() != expected_foreground_) ||
          !ListenerFocusedContextIs(focus_guard_, context_)) {
        MarkWritePrevented();
        hr = HRESULT_FROM_WIN32(ERROR_CANCELLED);
      } else {
        hr = InsertText(edit_cookie);
      }
      break;
    }
    case ListenerTypeCompositionOp::kStreamUpdate:
    case ListenerTypeCompositionOp::kStreamCommit:
    case ListenerTypeCompositionOp::kStreamCancel:
      hr = RunCompositionOp(edit_cookie);
      break;
    default:
      hr = E_UNEXPECTED;
      break;
  }
  if (async_state_) {
    async_state_->result = hr;
    if (async_state_->event != nullptr) {
      SetEvent(async_state_->event);
    }
  }
  return hr;
}

bool ListenerTypeEditSession::StopRequestedBeforeWrite() {
  if (!stop_signal_ || stop_signal_->event == nullptr) return false;
  // When an event was supplied, only a valid unsignalled event permits the
  // request to proceed. A missing named event is rejected before dispatch.
  if (WaitForSingleObject(stop_signal_->event, 0) == WAIT_TIMEOUT) {
    return false;
  }
  stop_signal_->prevented_write.store(true, std::memory_order_release);
  return true;
}

void ListenerTypeEditSession::MarkWritePrevented() {
  if (stop_signal_) stop_signal_->prevented_write.store(true, std::memory_order_release);
}

HRESULT ListenerTypeEditSession::InsertText(TfEditCookie edit_cookie) {
  if (context_ == nullptr) {
    return E_UNEXPECTED;
  }

  ITfInsertAtSelection* insert_at_selection = nullptr;
  HRESULT hr = context_->QueryInterface(IID_ITfInsertAtSelection,
                                        reinterpret_cast<void**>(
                                            &insert_at_selection));
  if (FAILED(hr)) {
    return hr;
  }

  ITfRange* query_range = nullptr;
  hr = insert_at_selection->InsertTextAtSelection(
      edit_cookie, TF_IAS_QUERYONLY, text_.c_str(),
      static_cast<LONG>(text_.size()), &query_range);
  if (query_range != nullptr) {
    query_range->Release();
    query_range = nullptr;
  }

  if (SUCCEEDED(hr)) {
    ITfRange* committed_range = nullptr;
    if (StopRequestedBeforeWrite()) {
      hr = HRESULT_FROM_WIN32(ERROR_CANCELLED);
    } else {
      if (stop_signal_) stop_signal_->write_attempted.store(true, std::memory_order_release);
      hr = insert_at_selection->InsertTextAtSelection(
          edit_cookie, 0, text_.c_str(), static_cast<LONG>(text_.size()),
          &committed_range);
    }
    if (committed_range != nullptr) {
      if (SUCCEEDED(hr)) {
        const HRESULT collapse_hr =
            committed_range->Collapse(edit_cookie, TF_ANCHOR_END);
        if (SUCCEEDED(collapse_hr)) {
          TF_SELECTION selection = {};
          selection.range = committed_range;
          selection.style.ase = TF_AE_END;
          selection.style.fInterimChar = FALSE;
          const HRESULT selection_hr = context_->SetSelection(edit_cookie, 1, &selection);
          // The text may already be present. Report an uncertain outcome for
          // guarded streaming rather than claim success and risk writing the
          // next segment at a stale or selected position.
          if ((op_ == ListenerTypeCompositionOp::kInsertGuarded ||
               op_ == ListenerTypeCompositionOp::kInsertGuardedTerminal) && FAILED(selection_hr)) {
            hr = selection_hr;
          }
        } else if (op_ == ListenerTypeCompositionOp::kInsertGuarded ||
                   op_ == ListenerTypeCompositionOp::kInsertGuardedTerminal) {
          hr = collapse_hr;
        }
      }
      committed_range->Release();
    } else if (SUCCEEDED(hr) && (op_ == ListenerTypeCompositionOp::kInsertGuarded ||
                                 op_ == ListenerTypeCompositionOp::kInsertGuardedTerminal)) {
      hr = E_UNEXPECTED;  // Insert may have happened, but its caret is unknown.
    }
  }

  insert_at_selection->Release();
  return hr;
}

HRESULT ListenerTypeEditSession::RunCompositionOp(TfEditCookie edit_cookie) {
  if (context_ == nullptr || composition_slot_ == nullptr) {
    return E_UNEXPECTED;
  }
  ITfComposition* composition = *composition_slot_;

  if (op_ == ListenerTypeCompositionOp::kStreamUpdate) {
    if (composition == nullptr) {
      // 开组字:QUERYONLY 拿到"将插入的位置"范围,在此范围上 StartComposition。
      // 文字参数在 QUERYONLY 下被忽略,这里按文档习惯传空。
      ITfInsertAtSelection* insert_at_selection = nullptr;
      HRESULT hr = context_->QueryInterface(
          IID_ITfInsertAtSelection, reinterpret_cast<void**>(&insert_at_selection));
      if (FAILED(hr)) {
        return hr;
      }
      ITfRange* insertion_range = nullptr;
      hr = insert_at_selection->InsertTextAtSelection(
          edit_cookie, TF_IAS_QUERYONLY, L"", 0, &insertion_range);
      insert_at_selection->Release();
      if (FAILED(hr) || insertion_range == nullptr) {
        return FAILED(hr) ? hr : E_FAIL;
      }

      ITfContextComposition* context_composition = nullptr;
      hr = context_->QueryInterface(
          IID_ITfContextComposition, reinterpret_cast<void**>(&context_composition));
      if (FAILED(hr)) {
        insertion_range->Release();
        return hr;
      }
      // 无 sink:我们不做组字期 UI 交互,只做文本呈现。
      hr = context_composition->StartComposition(
          edit_cookie, insertion_range, nullptr, &composition);
      context_composition->Release();
      insertion_range->Release();
      if (FAILED(hr) || composition == nullptr) {
        return FAILED(hr) ? hr : E_FAIL;
      }
      *composition_slot_ = composition;
    }
    // 原地替换组字内容:云端修订(改字/收缩)天然免费。
    ITfRange* range = nullptr;
    HRESULT hr = composition->GetRange(&range);
    if (FAILED(hr)) {
      return hr;
    }
    hr = range->SetText(edit_cookie, 0, text_.c_str(),
                        static_cast<LONG>(text_.size()));
    range->Release();
    return hr;
  }

  if (op_ == ListenerTypeCompositionOp::kStreamCommit) {
    if (composition == nullptr) {
      // 组字没开成(应用后进焦点/组字被应用吞掉):退化为一次性插入。
      return InsertText(edit_cookie);
    }
    ITfRange* range = nullptr;
    HRESULT hr = composition->GetRange(&range);
    if (SUCCEEDED(hr)) {
      hr = range->SetText(edit_cookie, 0, text_.c_str(),
                          static_cast<LONG>(text_.size()));
      range->Release();
    }
    if (FAILED(hr)) {
      return hr;
    }
    hr = composition->EndComposition(edit_cookie);
    composition->Release();
    *composition_slot_ = nullptr;
    return hr;
  }

  // kStreamCancel:清空并结束,文档不留字。
  if (composition == nullptr) {
    return S_OK;
  }
  ITfRange* range = nullptr;
  HRESULT hr = composition->GetRange(&range);
  if (SUCCEEDED(hr)) {
    hr = range->SetText(edit_cookie, 0, L"", 0);
    range->Release();
  }
  const HRESULT end_hr = composition->EndComposition(edit_cookie);
  composition->Release();
  *composition_slot_ = nullptr;
  return FAILED(hr) ? hr : end_hr;
}
