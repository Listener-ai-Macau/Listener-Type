#include "../src/pending_request_registry.h"

#include <cassert>
#include <condition_variable>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

struct Request {
  explicit Request(std::wstring value) : text(std::move(value)) {}
  std::wstring text;
};

int main() {
  PendingRequestRegistry<Request> registry;
  {
    auto sender = std::make_shared<Request>(L"never dispatched");
    std::weak_ptr<Request> weak = sender;
    const auto token = registry.Register(sender);
    assert(registry.Cancel(token));  // Never taken: definitely no dispatch.
    assert(!registry.Take(token));
    sender.reset();
    assert(weak.expired());
  }
  {
    auto sender = std::make_shared<Request>(L"中文 delayed 𝄞");
    std::weak_ptr<Request> weak = sender;
    const auto token = registry.Register(sender);
    std::mutex mutex;
    std::condition_variable condition;
    bool taken = false;
    bool release = false;
    std::thread receiver([&] {
      auto owned = registry.Take(token);
      assert(owned && owned->text == L"中文 delayed 𝄞");
      {
        std::unique_lock<std::mutex> lock(mutex);
        taken = true;
        condition.notify_one();
        condition.wait(lock, [&] { return release; });
      }
      assert(owned->text == L"中文 delayed 𝄞");
    });
    {
      std::unique_lock<std::mutex> lock(mutex);
      condition.wait(lock, [&] { return taken; });
    }
    // Simulate a SendMessageTimeout after the receiving thread started.
    assert(!registry.Cancel(token));  // Already taken: completion is unknown.
    assert(!registry.Take(token));  // A second delivery cannot replay it.
    sender.reset();
    assert(!weak.expired());  // Receiver still owns text after sender returns.
    {
      std::lock_guard<std::mutex> lock(mutex);
      release = true;
    }
    condition.notify_one();
    receiver.join();
    assert(weak.expired());
  }
}
