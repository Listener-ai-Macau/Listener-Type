#include <windows.h>
#include <msctf.h>
#include <objbase.h>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>
#include "../src/terminal_tail_guard.h"
#include "../src/edit_session.h"

LONG g_object_count=0;

class TerminalProbe final : public ITfEditSession {
 public:
  TerminalProbe(ITfContext* context, ITfThreadMgr* manager) : context_(context), manager_(manager) { context_->AddRef(); manager_->AddRef(); }
  ~TerminalProbe() { context_->Release(); manager_->Release(); }
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID id, void** out) override {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (id != IID_IUnknown && id != IID_ITfEditSession) return E_NOINTERFACE;
    *out = static_cast<ITfEditSession*>(this); AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs_); }
  ULONG STDMETHODCALLTYPE Release() override { ULONG n = InterlockedDecrement(&refs_); if (!n) delete this; return n; }

  bool Check(TfEditCookie ec, const std::wstring& expected) {
    return ListenerInspectTerminalTail(context_, ec, expected) == ListenerTerminalTail::kMatches;
  }

  bool ReplaceAndSelect(TfEditCookie ec, const std::wstring& text) {
    ITfRange* start = nullptr; ITfRange* end = nullptr;
    HRESULT hr = context_->GetStart(ec, &start);
    if (SUCCEEDED(hr)) hr = context_->GetEnd(ec, &end);
    if (SUCCEEDED(hr)) hr = start->ShiftEndToRange(ec, end, TF_ANCHOR_END);
    if (SUCCEEDED(hr)) hr = start->SetText(ec, 0, text.c_str(), static_cast<LONG>(text.size()));
    if (SUCCEEDED(hr)) hr = start->Collapse(ec, TF_ANCHOR_END);
    if (SUCCEEDED(hr)) { TF_SELECTION selection{}; selection.range=start; selection.style.ase=TF_AE_END; hr=context_->SetSelection(ec,1,&selection); }
    if (start) start->Release(); if (end) end->Release(); return SUCCEEDED(hr);
  }

  HRESULT STDMETHODCALLTYPE DoEditSession(TfEditCookie ec) override {
    const std::wstring body = L"合成 test 𝄞";
    if (!ReplaceAndSelect(ec, body)) return E_FAIL;
    valid_body = Check(ec, body);
    auto* terminal = new ListenerTypeEditSession(context_, L"。", ListenerTypeCompositionOp::kInsertGuardedTerminal,
                                                 nullptr, nullptr, manager_, body);
    const HRESULT inserted = terminal->DoEditSession(ec);
    terminal_inserted = SUCCEEDED(inserted) && inserted != S_FALSE && Check(ec, body + L"。");
    const HRESULT duplicate = terminal->DoEditSession(ec);
    duplicate_not_inserted = duplicate == S_FALSE && Check(ec, body + L"。");
    terminal->Release();
    if (!ReplaceAndSelect(ec, body)) return E_FAIL;
    ITfRange* selected = nullptr; ITfRange* doc_end = nullptr;
    HRESULT selection_hr = context_->GetStart(ec, &selected);
    if (SUCCEEDED(selection_hr)) selection_hr = context_->GetEnd(ec, &doc_end);
    if (SUCCEEDED(selection_hr)) selection_hr = selected->ShiftEndToRange(ec, doc_end, TF_ANCHOR_END);
    if (SUCCEEDED(selection_hr)) { TF_SELECTION selection{}; selection.range=selected; selection.style.ase=TF_AE_END; selection_hr=context_->SetSelection(ec,1,&selection); }
    if (selected) selected->Release(); if (doc_end) doc_end->Release();
    if (FAILED(selection_hr)) return selection_hr;
    selection_rejected = !Check(ec, body);
    terminal = new ListenerTypeEditSession(context_, L"。", ListenerTypeCompositionOp::kInsertGuardedTerminal,
                                           nullptr, nullptr, manager_, body);
    selection_not_written = FAILED(terminal->DoEditSession(ec)); terminal->Release();
    if (!ReplaceAndSelect(ec, body + L"。")) return E_FAIL;
    already_present_detected = Check(ec, body + L"。");
    already_present_not_appended = !Check(ec, body);
    if (!ReplaceAndSelect(ec, body)) return E_FAIL;
    ITfRange* start = nullptr; HRESULT hr = context_->GetStart(ec, &start);
    if (SUCCEEDED(hr)) { TF_SELECTION selection{}; selection.range=start; selection.style.ase=TF_AE_END; hr=context_->SetSelection(ec,1,&selection); }
    if (start) start->Release(); if (FAILED(hr)) return hr;
    moved_caret_rejected = !Check(ec, body);
    terminal = new ListenerTypeEditSession(context_, L"。", ListenerTypeCompositionOp::kInsertGuardedTerminal,
                                           nullptr, nullptr, manager_, body);
    moved_caret_not_written = FAILED(terminal->DoEditSession(ec)); terminal->Release();
    if (!ReplaceAndSelect(ec, L"已被用户改写")) return E_FAIL;
    changed_body_rejected = !Check(ec, body);
    terminal = new ListenerTypeEditSession(context_, L"。", ListenerTypeCompositionOp::kInsertGuardedTerminal,
                                           nullptr, nullptr, manager_, body);
    changed_body_not_written = FAILED(terminal->DoEditSession(ec)) && Check(ec, L"已被用户改写"); terminal->Release();
    if (!ReplaceAndSelect(ec, L"")) return E_FAIL;
    cleared_body_rejected = !Check(ec, body);
    return S_OK;
  }
  bool valid_body=false, terminal_inserted=false, duplicate_not_inserted=false, moved_caret_not_written=false;
  bool selection_not_written=false, changed_body_not_written=false;
  bool selection_rejected=false, already_present_detected=false, already_present_not_appended=false;
  bool moved_caret_rejected=false, changed_body_rejected=false, cleared_body_rejected=false;
 private:
  LONG refs_=1; ITfContext* context_; ITfThreadMgr* manager_;
};

int wmain(int argc, wchar_t** argv) {
  if (argc != 2) return 2;
  wchar_t station[256]{}; DWORD needed=0;
  if (!GetUserObjectInformationW(GetProcessWindowStation(), UOI_NAME, station, sizeof(station), &needed) ||
      std::wstring(station).find(L"ListenerNativeReplay_") != 0) return 3;
  if (FAILED(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED))) return 4;
  ITfThreadMgr* manager=nullptr; ITfDocumentMgr* document=nullptr; ITfContext* context=nullptr;
  TfClientId id=TF_CLIENTID_NULL; TfEditCookie cookie=0;
  HRESULT hr=CoCreateInstance(CLSID_TF_ThreadMgr,nullptr,CLSCTX_INPROC_SERVER,IID_ITfThreadMgr,reinterpret_cast<void**>(&manager));
  if (SUCCEEDED(hr)) hr=manager->Activate(&id);
  if (SUCCEEDED(hr)) hr=manager->CreateDocumentMgr(&document);
  if (SUCCEEDED(hr)) hr=document->CreateContext(id,0,nullptr,&context,&cookie);
  if (SUCCEEDED(hr)) hr=document->Push(context);
  if (SUCCEEDED(hr)) hr=manager->SetFocus(document);
  TerminalProbe* probe=context?new TerminalProbe(context,manager):nullptr; HRESULT edit_result=E_UNEXPECTED;
  HRESULT request=probe?context->RequestEditSession(id,probe,TF_ES_SYNC|TF_ES_READWRITE,&edit_result):E_FAIL;
  std::ofstream out{std::filesystem::path(argv[1])};
  out<<"{\"available\":true,\"setup\":"<<(SUCCEEDED(hr)?"true":"false")
     <<",\"edit_completed\":"<<((SUCCEEDED(request)&&SUCCEEDED(edit_result))?"true":"false")
     <<",\"valid_body\":"<<(probe&&probe->valid_body?"true":"false")
     <<",\"terminal_inserted\":"<<(probe&&probe->terminal_inserted?"true":"false")
     <<",\"duplicate_not_inserted\":"<<(probe&&probe->duplicate_not_inserted?"true":"false")
     <<",\"moved_caret_not_written\":"<<(probe&&probe->moved_caret_not_written?"true":"false")
     <<",\"selection_not_written\":"<<(probe&&probe->selection_not_written?"true":"false")
     <<",\"changed_body_not_written\":"<<(probe&&probe->changed_body_not_written?"true":"false")
     <<",\"selection_rejected\":"<<(probe&&probe->selection_rejected?"true":"false")
     <<",\"already_present_detected\":"<<(probe&&probe->already_present_detected?"true":"false")
     <<",\"already_present_not_appended\":"<<(probe&&probe->already_present_not_appended?"true":"false")
     <<",\"moved_caret_rejected\":"<<(probe&&probe->moved_caret_rejected?"true":"false")
     <<",\"changed_body_rejected\":"<<(probe&&probe->changed_body_rejected?"true":"false")
     <<",\"cleared_body_rejected\":"<<(probe&&probe->cleared_body_rejected?"true":"false")<<"}";
  out.close();
  const bool passed = SUCCEEDED(hr) && SUCCEEDED(request) && SUCCEEDED(edit_result) && probe &&
      probe->valid_body && probe->terminal_inserted && probe->duplicate_not_inserted &&
      probe->moved_caret_not_written && probe->selection_not_written && probe->changed_body_not_written &&
      probe->selection_rejected && probe->already_present_detected && probe->already_present_not_appended &&
      probe->moved_caret_rejected && probe->changed_body_rejected && probe->cleared_body_rejected;
  if(probe)probe->Release(); if(document)document->Pop(TF_POPF_ALL);
  if(context)context->Release(); if(document)document->Release(); if(manager){manager->Deactivate();manager->Release();}
  CoUninitialize(); return passed ? 0 : 5;
}
