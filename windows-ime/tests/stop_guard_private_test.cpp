#include <cstdint>
#include <filesystem>
#include <fstream>
#include <msctf.h>
#include <objbase.h>
#include <string>
#include <windows.h>
// Run only in a private ListenerNativeReplay_ window station. This replaces
// the foreground query for a synthetic target; it never drives a real window.
static HWND fake_foreground =
    reinterpret_cast<HWND>(static_cast<std::uintptr_t>(0x1234));
extern "C" HWND WINAPI FakeForeground() { return fake_foreground; }
extern "C" HWND(WINAPI *__imp_GetForegroundWindow)() = FakeForeground;
#include "../src/edit_session.h"
LONG g_object_count = 0;
class Reader final : public ITfEditSession {
public:
  LONG refs = 1;
  ITfContext *ctx;
  bool exact = false;
  explicit Reader(ITfContext *c) : ctx(c) { ctx->AddRef(); }
  ~Reader() { ctx->Release(); }
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID id, void **out) override {
    if (!out)
      return E_POINTER;
    *out = nullptr;
    if (id == IID_IUnknown || id == IID_ITfEditSession) {
      *out = static_cast<ITfEditSession *>(this);
      AddRef();
      return S_OK;
    }
    return E_NOINTERFACE;
  }
  ULONG STDMETHODCALLTYPE AddRef() override {
    return InterlockedIncrement(&refs);
  }
  ULONG STDMETHODCALLTYPE Release() override {
    ULONG n = InterlockedDecrement(&refs);
    if (!n)
      delete this;
    return n;
  }
  HRESULT STDMETHODCALLTYPE DoEditSession(TfEditCookie ec) override {
    ITfRange *start = nullptr;
    ITfRange *end = nullptr;
    HRESULT h = ctx->GetStart(ec, &start);
    if (SUCCEEDED(h))
      h = ctx->GetEnd(ec, &end);
    if (SUCCEEDED(h))
      h = start->ShiftEndToRange(ec, end, TF_ANCHOR_END);
    wchar_t buf[128]{};
    ULONG n = 0;
    if (SUCCEEDED(h))
      h = start->GetText(ec, 0, buf, 128, &n);
    if (SUCCEEDED(h))
      exact = std::wstring(buf, n) ==
              ((GetEnvironmentVariableW(L"LISTENER_PRIVATE_SIGNAL_STOP", nullptr,
                                       0) > 0 || GetEnvironmentVariableW(L"LISTENER_PRIVATE_DOCUMENT_MISMATCH", nullptr, 0) > 0)
                   ? L""
                   : L"合成 test 𝄞");
    if (start)
      start->Release();
    if (end)
      end->Release();
    return h;
  }
};
int wmain(int argc, wchar_t **argv) {
  if (argc != 2)
    return 2;
  wchar_t station[256]{};
  DWORD needed = 0;
  if (!GetUserObjectInformationW(GetProcessWindowStation(), UOI_NAME, station,
                                 sizeof(station), &needed) ||
      std::wstring(station).find(L"ListenerNativeReplay_") != 0)
    return 3;
  HRESULT ci = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  if (FAILED(ci))
    return 4;
  ITfThreadMgr *tm = nullptr;
  ITfDocumentMgr *doc = nullptr;
  ITfContext *ctx = nullptr;
  TfClientId id = TF_CLIENTID_NULL;
  TfEditCookie cookie = 0;
  HRESULT a =
      CoCreateInstance(CLSID_TF_ThreadMgr, nullptr, CLSCTX_INPROC_SERVER,
                       IID_ITfThreadMgr, reinterpret_cast<void **>(&tm));
  HRESULT b = tm ? tm->Activate(&id) : E_FAIL;
  HRESULT c = tm ? tm->CreateDocumentMgr(&doc) : E_FAIL;
  HRESULT d = doc ? doc->CreateContext(id, 0, nullptr, &ctx, &cookie) : E_FAIL;
  HRESULT e = (doc && ctx) ? doc->Push(ctx) : E_FAIL;
  HRESULT f = tm && doc ? tm->SetFocus(doc) : E_FAIL;
  ITfDocumentMgr *doc2 = nullptr;
  ITfContext *ctx2 = nullptr;
  TfEditCookie cookie2 = 0;
  HRESULT c2 = tm ? tm->CreateDocumentMgr(&doc2) : E_FAIL;
  HRESULT d2 =
      doc2 ? doc2->CreateContext(id, 0, nullptr, &ctx2, &cookie2) : E_FAIL;
  HRESULT e2 = doc2 && ctx2 ? doc2->Push(ctx2) : E_FAIL;
  HRESULT session_result = E_UNEXPECTED;
  HRESULT req = E_FAIL;
  bool exact = false;
  auto completion = std::make_shared<ListenerTypeAsyncEditState>();
  auto stop_signal = std::make_shared<ListenerTypeStopSignal>(
      CreateEventW(nullptr, TRUE, FALSE, nullptr));
  HRESULT focus2 = E_FAIL;
  bool callback = false;
  bool keep = false;
  bool stop_before_callback = false;
  const bool document_mismatch = GetEnvironmentVariableW(
      L"LISTENER_PRIVATE_DOCUMENT_MISMATCH", nullptr, 0) > 0;
  if (ctx) {
    auto *edit = new ListenerTypeEditSession(
        ctx, L"合成 test 𝄞", ListenerTypeCompositionOp::kInsertGuarded, nullptr,
        completion, tm, L"",
        reinterpret_cast<HWND>(static_cast<std::uintptr_t>(0x1234)),
        stop_signal, document_mismatch ? L"another document" : L"");
    req = ctx->RequestEditSession(id, edit, TF_ES_ASYNC | TF_ES_READWRITE,
                                  &session_result);
    edit->Release();
    focus2 = tm->SetFocus(doc);
    keep = true;
    stop_before_callback = GetEnvironmentVariableW(
                               L"LISTENER_PRIVATE_SIGNAL_STOP", nullptr, 0) > 0;
    if (stop_before_callback)
      SetEvent(stop_signal->event);
    DWORD begin = GetTickCount();
    while (GetTickCount() - begin < 3000) {
      MSG msg;
      while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
      }
      if (WaitForSingleObject(completion->event, 0) == WAIT_OBJECT_0) {
        callback = true;
        break;
      }
      Sleep(2);
    }
    auto *reader = new Reader(ctx);
    HRESULT read_result = E_UNEXPECTED;
    HRESULT read = ctx->RequestEditSession(id, reader, TF_ES_SYNC | TF_ES_READ,
                                           &read_result);
    exact = SUCCEEDED(read) && SUCCEEDED(read_result) && reader->exact;
    reader->Release();
  }
  std::ofstream out{std::filesystem::path(argv[1])};
  out << "{\"available\":true,\"scope\":\"private TSF context, synthetic text "
         "only\",\"manager_created\":"
      << (SUCCEEDED(a) ? "true" : "false")
      << ",\"manager_activated\":" << (SUCCEEDED(b) ? "true" : "false")
      << ",\"document_created\":" << (SUCCEEDED(c) ? "true" : "false")
      << ",\"context_created\":" << (SUCCEEDED(d) ? "true" : "false")
      << ",\"context_pushed\":" << (SUCCEEDED(e) ? "true" : "false")
      << ",\"document_focus\":" << (SUCCEEDED(f) ? "true" : "false")
      << ",\"edit_requested\":" << (SUCCEEDED(req) ? "true" : "false")
      << ",\"edit_result\":" << static_cast<unsigned long>(session_result)
      << ",\"simulated_stop_before_callback\":"
      << (stop_before_callback ? "true" : "false")
      << ",\"document_mismatch\":" << (document_mismatch ? "true" : "false")
      << ",\"callback_ran\":" << (callback ? "true" : "false")
      << ",\"result_matches\":"
      << ((callback &&
           ((stop_before_callback || document_mismatch)
                ? completion->result == HRESULT_FROM_WIN32(ERROR_CANCELLED)
                : SUCCEEDED(completion->result)))
              ? "true"
              : "false")
      << ",\"editor_matches_expected\":" << (exact ? "true" : "false")
      << ",\"prevented_write\":"
      << (stop_signal->prevented_write.load() ? "true" : "false")
      << ",\"second_context_ready\":"
      << ((SUCCEEDED(c2) && SUCCEEDED(d2) && SUCCEEDED(e2) && SUCCEEDED(focus2))
              ? "true"
              : "false")
      << "}";
  out.close();
  const bool passed =
      SUCCEEDED(a) && SUCCEEDED(b) && SUCCEEDED(c) && SUCCEEDED(d) &&
      SUCCEEDED(e) && SUCCEEDED(f) && SUCCEEDED(req) && callback && exact &&
      ((stop_before_callback || document_mismatch)
           ? completion->result == HRESULT_FROM_WIN32(ERROR_CANCELLED)
           : SUCCEEDED(completion->result)) &&
      stop_signal->prevented_write.load() == stop_before_callback;
  if (doc2 && SUCCEEDED(e2))
    doc2->Pop(TF_POPF_ALL);
  if (ctx2)
    ctx2->Release();
  if (doc2)
    doc2->Release();
  if (doc && SUCCEEDED(e))
    doc->Pop(TF_POPF_ALL);
  if (ctx)
    ctx->Release();
  if (doc)
    doc->Release();
  if (tm) {
    if (SUCCEEDED(b))
      tm->Deactivate();
    tm->Release();
  }
  CoUninitialize();
  return passed ? 0 : 1;
}
