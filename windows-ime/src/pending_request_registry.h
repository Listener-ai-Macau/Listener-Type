#pragma once

#include <cstdint>
#include <memory>
#include <mutex>
#include <unordered_map>
#include <utility>

// A window message carries an opaque token, never a borrowed pointer. The
// recipient takes shared ownership before processing, so sender timeout can
// remove an undelivered request without freeing one already being processed.
template <typename Request>
class PendingRequestRegistry {
 public:
  std::uintptr_t Register(const std::shared_ptr<Request>& request) {
    const auto token = reinterpret_cast<std::uintptr_t>(request.get());
    std::lock_guard<std::mutex> lock(mutex_);
    pending_.emplace(token, request);
    return token;
  }

  std::shared_ptr<Request> Take(std::uintptr_t token) {
    std::lock_guard<std::mutex> lock(mutex_);
    auto found = pending_.find(token);
    if (found == pending_.end()) return {};
    auto request = std::move(found->second);
    pending_.erase(found);
    return request;
  }

  bool Cancel(std::uintptr_t token) {
    std::lock_guard<std::mutex> lock(mutex_);
    return pending_.erase(token) != 0;
  }

 private:
  std::mutex mutex_;
  std::unordered_map<std::uintptr_t, std::shared_ptr<Request>> pending_;
};
