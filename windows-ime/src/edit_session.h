#pragma once

#include <msctf.h>
#include <atomic>
#include <memory>
#include <optional>
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

// One request holds its own reference to the OS event through a delayed edit.
// The flag proves that a callback returned before attempting a write; it is
// not proof that an already-started insertion can be cancelled. A null event
// means this request has no stop signal but can still report a proven guard.
struct ListenerTypeStopSignal {
  explicit ListenerTypeStopSignal(HANDLE handle) : event(handle) {}
  ListenerTypeStopSignal(const ListenerTypeStopSignal&) = delete;
  ListenerTypeStopSignal& operator=(const ListenerTypeStopSignal&) = delete;
  ~ListenerTypeStopSignal() { if (event != nullptr) CloseHandle(event); }
  HANDLE event = nullptr;
  std::atomic<bool> prevented_write{false};
  std::atomic<bool> write_attempted{false};
};

inline bool ListenerWriteDefinitelyPrevented(const std::shared_ptr<ListenerTypeStopSignal>& proof) {
  return proof && proof->prevented_write.load(std::memory_order_acquire) &&
         !proof->write_attempted.load(std::memory_order_acquire);
}

// 组字流式(2026-09-22 讯飞式逐字上屏)的操作类型。kInsertOnce 是既有的一次性
// 插入路径,行为不变;其余三个维护 service 持有的那条组字:
//   kStreamUpdate  无组字 → 选区处 StartComposition 并写入;有 → 原地替换。
//   kStreamCommit  组字内容替换为终稿 → EndComposition 落定;无组字则退化
//                  为一次性插入(兜底:应用后进焦点等情况)。
//   kStreamCancel  组字清空 → EndComposition(文档不留字);无组字则无操作。
enum class ListenerTypeCompositionOp {
  kInsertOnce,
  kBindDirectTarget,
  kInsertGuarded,
  kInsertGuardedTerminal,
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
      std::shared_ptr<ListenerTypeAsyncEditState> async_state = nullptr,
      ITfThreadMgr* focus_guard = nullptr,
      std::wstring expected_body = {},
      HWND expected_foreground = nullptr,
      std::shared_ptr<ListenerTypeStopSignal> stop_signal = nullptr,
      std::optional<std::wstring> expected_document = std::nullopt);
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
  bool StopRequestedBeforeWrite();
  void MarkWritePrevented();

  LONG ref_count_ = 1;
  ITfContext* context_ = nullptr;
  std::wstring text_;
  ListenerTypeCompositionOp op_ = ListenerTypeCompositionOp::kInsertOnce;
  // service 持有的组字指针的地址(in/out):开组字时写入,commit/cancel 时
  // 清空释放。仅在 owner 线程的 edit session 里访问,无需加锁。
  ITfComposition** composition_slot_ = nullptr;
  std::shared_ptr<ListenerTypeAsyncEditState> async_state_;
  ITfThreadMgr* focus_guard_ = nullptr;
  std::wstring expected_body_;
  HWND expected_foreground_ = nullptr;
  std::shared_ptr<ListenerTypeStopSignal> stop_signal_;
  std::optional<std::wstring> expected_document_;
};
