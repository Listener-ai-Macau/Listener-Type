#include "text_service.h"

#include <atomic>
#include <cerrno>
#include <cstdint>
#include <cwchar>
#include <limits>
#include <memory>
#include <new>

#include "edit_session.h"
#include "focus_context_guard.h"
#include "pending_request_registry.h"

extern LONG g_object_count;
extern HINSTANCE g_module;

namespace {

constexpr wchar_t kMessageWindowClassName[] = L"ListenerTypeImeMessageWindow";
constexpr UINT kSubmitTextMessage = WM_APP + 1;
constexpr UINT kSubmitTextTimeoutMs = 3000;

struct SubmitTextRequest {
  std::wstring session_id;
  std::wstring text;
  std::wstring expected_body;
  ListenerTypeCompositionOp op = ListenerTypeCompositionOp::kInsertOnce;
  std::shared_ptr<ListenerTypeAsyncEditState> async_completion;
  bool wait_for_async_completion = false;
  bool edit_requested = false;
  std::atomic<bool> handler_finished{false};
  HRESULT result = E_UNEXPECTED;
};

PendingRequestRegistry<SubmitTextRequest> g_submit_requests;

// New guarded sessions encode the original foreground HWND in a unique token.
// Older experimental tokens intentionally keep their old behavior for private
// regression fixtures; production requires a version capability probe first.
bool ParseForegroundToken(const std::wstring& token, HWND* expected) {
  *expected = nullptr;
  if (token.rfind(L"fg:", 0) != 0) return true;
  const size_t end = token.find(L':', 3);
  if (end == std::wstring::npos || end == 3 || end + 1 >= token.size()) return false;
  const std::wstring value = token.substr(3, end - 3);
  if (value.find_first_not_of(L"0123456789abcdefABCDEF") != std::wstring::npos) return false;
  wchar_t* tail = nullptr;
  errno = 0;
  const unsigned long long parsed = std::wcstoull(value.c_str(), &tail, 16);
  if (errno == ERANGE || tail == value.c_str() || *tail != L'\0' || parsed == 0 ||
      parsed > static_cast<unsigned long long>((std::numeric_limits<std::uintptr_t>::max)())) return false;
  *expected = reinterpret_cast<HWND>(static_cast<std::uintptr_t>(parsed));
  return true;
}

HRESULT WaitForAsyncEditCompletion(
    const std::shared_ptr<ListenerTypeAsyncEditState>& completion) {
  if (!completion || !completion->IsValid()) {
    return HRESULT_FROM_WIN32(completion && completion->create_error != ERROR_SUCCESS
                                  ? completion->create_error
                                  : ERROR_INVALID_HANDLE);
  }

  const DWORD wait_result =
      WaitForSingleObject(completion->event, kSubmitTextTimeoutMs);
  if (wait_result == WAIT_OBJECT_0) {
    return completion->result;
  }
  if (wait_result == WAIT_TIMEOUT) {
    return HRESULT_FROM_WIN32(ERROR_TIMEOUT);
  }
  return HRESULT_FROM_WIN32(GetLastError());
}

}  // namespace

ListenerTypeTextService::ListenerTypeTextService() {
  InterlockedIncrement(&g_object_count);
}

ListenerTypeTextService::~ListenerTypeTextService() {
  Deactivate();
  InterlockedDecrement(&g_object_count);
}

STDMETHODIMP ListenerTypeTextService::QueryInterface(REFIID iid, void** object) {
  if (object == nullptr) {
    return E_POINTER;
  }
  *object = nullptr;

  if (iid == IID_IUnknown || iid == IID_ITfTextInputProcessor ||
      iid == IID_ITfTextInputProcessorEx) {
    *object = static_cast<ITfTextInputProcessorEx*>(this);
    AddRef();
    return S_OK;
  }

  return E_NOINTERFACE;
}

STDMETHODIMP_(ULONG) ListenerTypeTextService::AddRef() {
  return static_cast<ULONG>(InterlockedIncrement(&ref_count_));
}

STDMETHODIMP_(ULONG) ListenerTypeTextService::Release() {
  const ULONG count = static_cast<ULONG>(InterlockedDecrement(&ref_count_));
  if (count == 0) {
    delete this;
  }
  return count;
}

STDMETHODIMP ListenerTypeTextService::Activate(ITfThreadMgr* thread_mgr,
                                           TfClientId client_id) {
  return ActivateEx(thread_mgr, client_id, 0);
}

STDMETHODIMP ListenerTypeTextService::ActivateEx(ITfThreadMgr* thread_mgr,
                                             TfClientId client_id,
                                             DWORD flags) {
  UNREFERENCED_PARAMETER(flags);

  if (thread_mgr == nullptr) {
    return E_INVALIDARG;
  }

  Deactivate();

  owner_thread_id_ = GetCurrentThreadId();

  thread_mgr_ = thread_mgr;
  thread_mgr_->AddRef();
  client_id_ = client_id;

  HRESULT hr = EnsureMessageWindow();
  if (FAILED(hr)) {
    Deactivate();
    return hr;
  }

  hr = StartIpcServer();
  if (FAILED(hr)) {
    Deactivate();
    return hr;
  }

  return S_OK;
}

STDMETHODIMP ListenerTypeTextService::Deactivate() {
  StopIpcServer();
  DestroyMessageWindow();
  DropActiveComposition();
  if (bound_direct_context_ != nullptr) {
    bound_direct_context_->Release();
    bound_direct_context_ = nullptr;
  }
  bound_direct_token_.clear();

  if (thread_mgr_ != nullptr) {
    thread_mgr_->Release();
    thread_mgr_ = nullptr;
  }
  client_id_ = TF_CLIENTID_NULL;
  owner_thread_id_ = 0;

  return S_OK;
}

HRESULT ListenerTypeTextService::SubmitTextFromPipe(
    const std::wstring& session_id,
    const std::wstring& text,
    bool* definitely_not_dispatched) {
  return SubmitEditOnOwnerThread(session_id, text,
                                 ListenerTypeCompositionOp::kInsertOnce,
                                 definitely_not_dispatched);
}

HRESULT ListenerTypeTextService::BindDirectTargetFromPipe(
    const std::wstring& token, bool* definitely_not_dispatched) {
  return SubmitEditOnOwnerThread(token, L"", ListenerTypeCompositionOp::kBindDirectTarget,
                                 definitely_not_dispatched);
}

HRESULT ListenerTypeTextService::SubmitGuardedFromPipe(
    const std::wstring& token, const std::wstring& text, bool* definitely_not_dispatched) {
  return SubmitEditOnOwnerThread(token, text, ListenerTypeCompositionOp::kInsertGuarded,
                                 definitely_not_dispatched);
}

HRESULT ListenerTypeTextService::SubmitTerminalFromPipe(
    const std::wstring& token, const std::wstring& text, const std::wstring& expected_body,
    bool* definitely_not_dispatched) {
  return SubmitEditOnOwnerThread(token, text, ListenerTypeCompositionOp::kInsertGuardedTerminal,
                                 definitely_not_dispatched, expected_body);
}

HRESULT ListenerTypeTextService::StreamCompositionFromPipe(
    const std::wstring& session_id,
    const std::wstring& text,
    ListenerTypeCompositionOp op,
    bool* definitely_not_dispatched) {
  return SubmitEditOnOwnerThread(session_id, text, op, definitely_not_dispatched);
}

HRESULT ListenerTypeTextService::SubmitEditOnOwnerThread(
    const std::wstring& session_id,
    const std::wstring& text,
    ListenerTypeCompositionOp op,
    bool* definitely_not_dispatched,
    const std::wstring& expected_body) {
  if (definitely_not_dispatched == nullptr) return E_POINTER;
  *definitely_not_dispatched = true;
  if (GetCurrentThreadId() == owner_thread_id_) {
    bool requested = false;
    const HRESULT result = CommitTextOnOwnerThread(session_id, text, op, expected_body, nullptr, nullptr, &requested);
    *definitely_not_dispatched = !requested;
    return result;
  }

  if (message_window_ == nullptr) {
    return E_UNEXPECTED;
  }

  auto request = std::make_shared<SubmitTextRequest>();
  request->session_id = session_id;
  request->text = text;
  request->expected_body = expected_body;
  request->op = op;
  const auto token = g_submit_requests.Register(request);
  DWORD_PTR message_result = 0;
  const LRESULT sent = SendMessageTimeoutW(
      message_window_, kSubmitTextMessage, static_cast<WPARAM>(token),
      0, SMTO_ABORTIFHUNG,
      kSubmitTextTimeoutMs, &message_result);
  const DWORD send_error = sent == 0 ? GetLastError() : ERROR_SUCCESS;
  // If undelivered, cancel the registry entry. A handler which already took
  // the request retains its own shared reference until it actually returns.
  const bool not_taken = g_submit_requests.Cancel(token);
  if (sent == 0) {
    // If the handler already took ownership it may still edit after timeout.
    *definitely_not_dispatched = not_taken;
    return HRESULT_FROM_WIN32(send_error != ERROR_SUCCESS ? send_error : ERROR_TIMEOUT);
  }

  // A receiver can release a synchronous sender early with ReplyMessage.
  // Do not read mutable request fields until our handler itself has finished.
  if (!request->handler_finished.load(std::memory_order_acquire)) {
    *definitely_not_dispatched = false;
    return HRESULT_FROM_WIN32(ERROR_TIMEOUT);
  }

  *definitely_not_dispatched = !request->edit_requested;

  if (request->wait_for_async_completion) {
    return WaitForAsyncEditCompletion(request->async_completion);
  }

  return request->result;
}

void ListenerTypeTextService::DropActiveComposition() {
  if (active_composition_ != nullptr) {
    // Deactivate 清场:没有 edit cookie 可用,只释放引用,残留的下划线文本
    // 由 TSF 在 context 销毁时回收。
    active_composition_->Release();
    active_composition_ = nullptr;
  }
}

HRESULT ListenerTypeTextService::StartIpcServer() {
  pipe_server_.Start(this);
  return S_OK;
}

void ListenerTypeTextService::StopIpcServer() {
  pipe_server_.Stop();
}

HRESULT ListenerTypeTextService::EnsureMessageWindow() {
  if (message_window_ != nullptr) {
    return S_OK;
  }

  WNDCLASSW window_class = {};
  window_class.lpfnWndProc = ListenerTypeTextService::MessageWindowProc;
  window_class.hInstance = g_module;
  window_class.lpszClassName = kMessageWindowClassName;

  if (!RegisterClassW(&window_class)) {
    const DWORD error = GetLastError();
    if (error != ERROR_CLASS_ALREADY_EXISTS) {
      return HRESULT_FROM_WIN32(error);
    }
  }

  message_window_ =
      CreateWindowExW(0, kMessageWindowClassName, L"", 0, 0, 0, 0, 0,
                      HWND_MESSAGE, nullptr, g_module, this);
  if (message_window_ == nullptr) {
    return HRESULT_FROM_WIN32(GetLastError());
  }

  return S_OK;
}

void ListenerTypeTextService::DestroyMessageWindow() {
  if (message_window_ != nullptr) {
    DestroyWindow(message_window_);
    message_window_ = nullptr;
  }
}

HRESULT ListenerTypeTextService::CommitTextOnOwnerThread(
    const std::wstring& session_id,
    const std::wstring& text,
    ListenerTypeCompositionOp op,
    const std::wstring& expected_body,
    std::shared_ptr<ListenerTypeAsyncEditState>* async_completion,
    bool* wait_for_async_completion,
    bool* edit_requested) {
  if (thread_mgr_ == nullptr || client_id_ == TF_CLIENTID_NULL) {
    return E_UNEXPECTED;
  }

  HWND expected_foreground = nullptr;
  if (op == ListenerTypeCompositionOp::kBindDirectTarget ||
      op == ListenerTypeCompositionOp::kInsertGuarded ||
      op == ListenerTypeCompositionOp::kInsertGuardedTerminal) {
    if (!ParseForegroundToken(session_id, &expected_foreground)) return E_INVALIDARG;
    if (expected_foreground != nullptr && GetForegroundWindow() != expected_foreground) {
      return HRESULT_FROM_WIN32(ERROR_CANCELLED);
    }
  }

  ITfDocumentMgr* document_mgr = nullptr;
  HRESULT hr = thread_mgr_->GetFocus(&document_mgr);
  if (FAILED(hr)) {
    return (op == ListenerTypeCompositionOp::kInsertGuarded || op == ListenerTypeCompositionOp::kInsertGuardedTerminal)
               ? HRESULT_FROM_WIN32(ERROR_CANCELLED) : hr;
  }
  if (document_mgr == nullptr) {
    return (op == ListenerTypeCompositionOp::kInsertGuarded || op == ListenerTypeCompositionOp::kInsertGuardedTerminal)
               ? HRESULT_FROM_WIN32(ERROR_CANCELLED) : E_FAIL;
  }

  ITfContext* context = nullptr;
  hr = document_mgr->GetTop(&context);
  document_mgr->Release();
  document_mgr = nullptr;
  if (FAILED(hr)) {
    return (op == ListenerTypeCompositionOp::kInsertGuarded || op == ListenerTypeCompositionOp::kInsertGuardedTerminal)
               ? HRESULT_FROM_WIN32(ERROR_CANCELLED) : hr;
  }
  if (context == nullptr) {
    return (op == ListenerTypeCompositionOp::kInsertGuarded || op == ListenerTypeCompositionOp::kInsertGuardedTerminal)
               ? HRESULT_FROM_WIN32(ERROR_CANCELLED) : E_FAIL;
  }

  if (op == ListenerTypeCompositionOp::kBindDirectTarget) {
    if (session_id.empty()) { context->Release(); return E_INVALIDARG; }
    if (bound_direct_context_ != nullptr) bound_direct_context_->Release();
    bound_direct_context_ = context;  // Retain GetTop's reference on this thread.
    bound_direct_token_ = session_id;
    return S_OK;
  }

  if (op == ListenerTypeCompositionOp::kInsertGuarded || op == ListenerTypeCompositionOp::kInsertGuardedTerminal) {
    const bool matches = !session_id.empty() && session_id == bound_direct_token_ &&
                         ListenerSameComIdentity(context, bound_direct_context_);
    if (bound_direct_context_ != nullptr) bound_direct_context_->Release();
    bound_direct_context_ = nullptr;
    bound_direct_token_.clear();
    if (!matches) { context->Release(); return HRESULT_FROM_WIN32(ERROR_CANCELLED); }
  }

  // 组字操作与组字指针只对非一次性路径生效;一次性插入保持原行为。
  ITfComposition** composition_slot =
      (op == ListenerTypeCompositionOp::kInsertOnce || op == ListenerTypeCompositionOp::kInsertGuarded ||
       op == ListenerTypeCompositionOp::kInsertGuardedTerminal)
          ? nullptr : &active_composition_;
  auto* session = new (std::nothrow) ListenerTypeEditSession(
      context, text, op, composition_slot, nullptr,
      (op == ListenerTypeCompositionOp::kInsertGuarded || op == ListenerTypeCompositionOp::kInsertGuardedTerminal)
          ? thread_mgr_ : nullptr, expected_body, expected_foreground);
  if (session == nullptr) {
    context->Release();
    return E_OUTOFMEMORY;
  }

  HRESULT edit_result = S_OK;
  if (edit_requested != nullptr) *edit_requested = true;
  hr = context->RequestEditSession(client_id_, session,
                                   TF_ES_SYNC | TF_ES_READWRITE, &edit_result);
  session->Release();

  const bool synchronous_rejected =
      hr == TF_E_SYNCHRONOUS ||
      (SUCCEEDED(hr) && edit_result == TF_E_SYNCHRONOUS);
  if (!synchronous_rejected) {
    context->Release();
    if (FAILED(hr)) {
      return hr;
    }
    return edit_result;
  }

  if (async_completion == nullptr || wait_for_async_completion == nullptr) {
    context->Release();
    if (FAILED(hr)) {
      return hr;
    }
    return edit_result;
  }

  auto completion = std::make_shared<ListenerTypeAsyncEditState>();
  if (!completion->IsValid()) {
    context->Release();
    return HRESULT_FROM_WIN32(completion->create_error != ERROR_SUCCESS
                                  ? completion->create_error
                                  : ERROR_INVALID_HANDLE);
  }

  auto* async_session = new (std::nothrow) ListenerTypeEditSession(
      context, text, op, composition_slot, completion,
      (op == ListenerTypeCompositionOp::kInsertGuarded || op == ListenerTypeCompositionOp::kInsertGuardedTerminal)
          ? thread_mgr_ : nullptr, expected_body, expected_foreground);
  if (async_session == nullptr) {
    context->Release();
    return E_OUTOFMEMORY;
  }

  HRESULT async_edit_result = S_OK;
  hr = context->RequestEditSession(client_id_, async_session,
                                   TF_ES_ASYNC | TF_ES_READWRITE,
                                   &async_edit_result);
  async_session->Release();
  context->Release();

  if (FAILED(hr)) {
    return hr;
  }
  if (FAILED(async_edit_result)) {
    return async_edit_result;
  }

  *async_completion = std::move(completion);
  *wait_for_async_completion = true;
  return S_OK;
}

LRESULT CALLBACK ListenerTypeTextService::MessageWindowProc(HWND window,
                                                        UINT message,
                                                        WPARAM wparam,
                                                        LPARAM lparam) {
  if (message == WM_NCCREATE) {
    const auto* create = reinterpret_cast<CREATESTRUCTW*>(lparam);
    SetWindowLongPtrW(window, GWLP_USERDATA,
                      reinterpret_cast<LONG_PTR>(create->lpCreateParams));
    return TRUE;
  }

  auto* service = reinterpret_cast<ListenerTypeTextService*>(
      GetWindowLongPtrW(window, GWLP_USERDATA));
  if (message == kSubmitTextMessage && service != nullptr) {
    auto request = g_submit_requests.Take(static_cast<std::uintptr_t>(wparam));
    if (!request) {
      return 0;
    }

    request->result = service->CommitTextOnOwnerThread(
        request->session_id, request->text, request->op, request->expected_body,
        &request->async_completion, &request->wait_for_async_completion,
        &request->edit_requested);
    request->handler_finished.store(true, std::memory_order_release);
    return 1;
  }

  return DefWindowProcW(window, message, wparam, lparam);
}
