#pragma once

#include <msctf.h>
#include <memory>
#include <string>
#include <windows.h>

struct ListenerTypeAsyncEditState {
  ListenerTypeAsyncEditState();
  ListenerTypeAsyncEditState(const ListenerTypeAsyncEditState&) = delete;
  ListenerTypeAsyncEditState& operator=(const ListenerTypeAsyncEditState&) = delete;
  ~ListenerTypeAsyncEditState();

  bool IsValid() const;

  HANDLE event = nullptr;
  DWORD create_error = ERROR_SUCCESS;
  HRESULT result = E_UNEXPECTED;
};

class ListenerTypeEditSession final : public ITfEditSession {
 public:
  ListenerTypeEditSession(
      ITfContext* context,
      std::wstring text,
      std::shared_ptr<ListenerTypeAsyncEditState> async_state = nullptr);
  ListenerTypeEditSession(const ListenerTypeEditSession&) = delete;
  ListenerTypeEditSession& operator=(const ListenerTypeEditSession&) = delete;
  ~ListenerTypeEditSession();

  STDMETHODIMP QueryInterface(REFIID iid, void** object) override;
  STDMETHODIMP_(ULONG) AddRef() override;
  STDMETHODIMP_(ULONG) Release() override;
  STDMETHODIMP DoEditSession(TfEditCookie edit_cookie) override;

 private:
  HRESULT InsertText(TfEditCookie edit_cookie);

  LONG ref_count_ = 1;
  ITfContext* context_ = nullptr;
  std::wstring text_;
  std::shared_ptr<ListenerTypeAsyncEditState> async_state_;
};
