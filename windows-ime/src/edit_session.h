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

// 组字流式(2026-09-22 讯飞式逐字上屏)的操作类型。kInsertOnce 是既有的一次性
// 插入路径,行为不变;其余三个维护 service 持有的那条组字:
//   kStreamUpdate  无组字 → 选区处 StartComposition 并写入;有 → 原地替换。
//   kStreamCommit  组字内容替换为终稿 → EndComposition 落定;无组字则退化
//                  为一次性插入(兜底:应用后进焦点等情况)。
//   kStreamCancel  组字清空 → EndComposition(文档不留字);无组字则无操作。
enum class ListenerTypeCompositionOp {
  kInsertOnce,
  kStreamUpdate,
  kStreamCommit,
  kStreamCancel,
};

class ListenerTypeEditSession final : public ITfEditSession {
 public:
  ListenerTypeEditSession(
      ITfContext* context,
      std::wstring text,
      ListenerTypeCompositionOp op = ListenerTypeCompositionOp::kInsertOnce,
      ITfComposition** composition_slot = nullptr,
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
  HRESULT RunCompositionOp(TfEditCookie edit_cookie);

  LONG ref_count_ = 1;
  ITfContext* context_ = nullptr;
  std::wstring text_;
  ListenerTypeCompositionOp op_ = ListenerTypeCompositionOp::kInsertOnce;
  // service 持有的组字指针的地址(in/out):开组字时写入,commit/cancel 时
  // 清空释放。仅在 owner 线程的 edit session 里访问,无需加锁。
  ITfComposition** composition_slot_ = nullptr;
  std::shared_ptr<ListenerTypeAsyncEditState> async_state_;
};
