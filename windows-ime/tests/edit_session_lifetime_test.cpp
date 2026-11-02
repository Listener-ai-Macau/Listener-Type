#include "../src/edit_session.h"

#include <cassert>

LONG g_object_count = 0;

int main() {
  auto* edit = new ListenerTypeEditSession(nullptr, L"synthetic");
  assert(g_object_count == 1);
  edit->AddRef();  // Model TSF retaining the edit after the caller returns.
  edit->Release();
  assert(g_object_count == 1);
  edit->Release();
  assert(g_object_count == 0);
}
