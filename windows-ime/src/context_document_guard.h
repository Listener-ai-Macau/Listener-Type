#pragma once

#include <msctf.h>
#include <string>
#include <vector>

// Called under the target context's edit lock. Text is never logged. This
// rejects a different document, but identical documents can still collide;
// it is therefore a necessary check, not proof of a unique editor identity.
inline bool ListenerContextDocumentEquals(ITfContext* context, TfEditCookie cookie,
                                          const std::wstring& expected) {
  if (context == nullptr || expected.size() > 16384) return false;
  ITfRange* start = nullptr;
  ITfRange* end = nullptr;
  HRESULT hr = context->GetStart(cookie, &start);
  if (SUCCEEDED(hr)) hr = context->GetEnd(cookie, &end);
  if (SUCCEEDED(hr) && start != nullptr && end != nullptr)
    hr = start->ShiftEndToRange(cookie, end, TF_ANCHOR_END);
  else if (SUCCEEDED(hr))
    hr = E_UNEXPECTED;
  std::vector<wchar_t> actual(expected.size() + 1);
  ULONG count = 0;
  if (SUCCEEDED(hr)) hr = start->GetText(cookie, 0, actual.data(),
                                         static_cast<ULONG>(actual.size()), &count);
  if (end != nullptr) end->Release();
  if (start != nullptr) start->Release();
  return SUCCEEDED(hr) && std::wstring(actual.data(), count) == expected;
}
