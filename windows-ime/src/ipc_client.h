#pragma once

#include <atomic>
#include <mutex>
#include <string>
#include <thread>
#include <windows.h>

class ListenerTypeTextService;

class ListenerTypePipeServer {
 public:
  ListenerTypePipeServer();
  ListenerTypePipeServer(const ListenerTypePipeServer&) = delete;
  ListenerTypePipeServer& operator=(const ListenerTypePipeServer&) = delete;
  ~ListenerTypePipeServer();

  void Start(ListenerTypeTextService* service);
  void Stop();

 private:
  void Run();
  bool ReadJsonLine(HANDLE pipe, std::string* line);
  void HandleSubmitLine(HANDLE pipe, const std::string& line);
  bool WriteResult(HANDLE pipe,
                   const std::wstring& session_id,
                   const wchar_t* status,
                   const wchar_t* error_code);
  void WakePipe();

  std::atomic<bool> stop_requested_{false};
  std::thread thread_;
  std::mutex pipe_mutex_;
  HANDLE pipe_handle_ = INVALID_HANDLE_VALUE;
  std::wstring pipe_name_;
  ListenerTypeTextService* service_ = nullptr;
};
