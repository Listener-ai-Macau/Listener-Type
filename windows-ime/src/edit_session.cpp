#include "edit_session.h"

#include <utility>

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
    std::shared_ptr<ListenerTypeAsyncEditState> async_state)
    : context_(context),
      text_(std::move(text)),
      op_(op),
      composition_slot_(composition_slot),
      async_state_(std::move(async_state)) {
  if (context_ != nullptr) {
    context_->AddRef();
  }
}

ListenerTypeEditSession::~ListenerTypeEditSession() {
  if (context_ != nullptr) {
    context_->Release();
    context_ = nullptr;
  }
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
  switch (op_) {
    case ListenerTypeCompositionOp::kInsertOnce:
      hr = InsertText(edit_cookie);
      break;
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
    hr = insert_at_selection->InsertTextAtSelection(
        edit_cookie, 0, text_.c_str(), static_cast<LONG>(text_.size()),
        &committed_range);
    if (committed_range != nullptr) {
      if (SUCCEEDED(hr)) {
        const HRESULT collapse_hr =
            committed_range->Collapse(edit_cookie, TF_ANCHOR_END);
        if (SUCCEEDED(collapse_hr)) {
          TF_SELECTION selection = {};
          selection.range = committed_range;
          selection.style.ase = TF_AE_END;
          selection.style.fInterimChar = FALSE;
          (void)context_->SetSelection(edit_cookie, 1, &selection);
        }
      }
      committed_range->Release();
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
