#pragma once

#include <msctf.h>
#include <memory>
#include <string>
#include <windows.h>

#include "edit_session.h"
#include "ipc_client.h"

struct ListenerTypeAsyncEditState;

class ListenerTypeTextService final : public ITfTextInputProcessorEx {
 public:
  ListenerTypeTextService();
  ListenerTypeTextService(const ListenerTypeTextService&) = delete;
  ListenerTypeTextService& operator=(const ListenerTypeTextService&) = delete;
  ~ListenerTypeTextService();

  STDMETHODIMP QueryInterface(REFIID iid, void** object) override;
  STDMETHODIMP_(ULONG) AddRef() override;
  STDMETHODIMP_(ULONG) Release() override;

  STDMETHODIMP Activate(ITfThreadMgr* thread_mgr, TfClientId client_id) override;
  STDMETHODIMP Deactivate() override;
  STDMETHODIMP ActivateEx(ITfThreadMgr* thread_mgr,
                          TfClientId client_id,
                          DWORD flags) override;

  HRESULT SubmitTextFromPipe(const std::wstring& session_id,
                             const std::wstring& text);
  // 组字流式(2026-09-22 讯飞式):update 原地替换组字内容;commit 终稿落定;
  // cancel 清空。会话切换时旧组字先 cancel。
  HRESULT StreamCompositionFromPipe(const std::wstring& session_id,
                                    const std::wstring& text,
                                    ListenerTypeCompositionOp op);

 private:
  HRESULT StartIpcServer();
  void StopIpcServer();
  HRESULT EnsureMessageWindow();
  void DestroyMessageWindow();
  HRESULT SubmitEditOnOwnerThread(const std::wstring& session_id,
                                  const std::wstring& text,
                                  ListenerTypeCompositionOp op);
  HRESULT CommitTextOnOwnerThread(
      const std::wstring& session_id,
      const std::wstring& text,
      ListenerTypeCompositionOp op,
      std::shared_ptr<ListenerTypeAsyncEditState>* async_completion,
      bool* wait_for_async_completion);
  // 释放当前组字(Deactivate 清场用;不在 edit cookie 内,只做 Release,
  // 由 TSF 在 context 销毁时回收)。
  void DropActiveComposition();

  static LRESULT CALLBACK MessageWindowProc(HWND window,
                                            UINT message,
                                            WPARAM wparam,
                                            LPARAM lparam);

  LONG ref_count_ = 1;
  ITfThreadMgr* thread_mgr_ = nullptr;
  TfClientId client_id_ = TF_CLIENTID_NULL;
  DWORD owner_thread_id_ = 0;
  HWND message_window_ = nullptr;
  ListenerTypePipeServer pipe_server_;
  // 仅 owner 线程访问(edit session 也在 owner 线程跑),无需加锁。会话边界
  // 由 Rust 驱动保证:新会话首个 stream_update 前必发 stream_cancel。
  ITfComposition* active_composition_ = nullptr;
};
