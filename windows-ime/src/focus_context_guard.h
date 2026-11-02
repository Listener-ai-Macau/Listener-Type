#pragma once

#include <msctf.h>

// COM interface pointers are not necessarily identity pointers. Compare the
// canonical IUnknown references while keeping both objects alive.
inline bool ListenerSameComIdentity(IUnknown* left, IUnknown* right) {
  if (left == nullptr || right == nullptr) return false;
  IUnknown* left_identity = nullptr;
  IUnknown* right_identity = nullptr;
  const HRESULT left_hr = left->QueryInterface(IID_IUnknown, reinterpret_cast<void**>(&left_identity));
  const HRESULT right_hr = right->QueryInterface(IID_IUnknown, reinterpret_cast<void**>(&right_identity));
  const bool same = SUCCEEDED(left_hr) && SUCCEEDED(right_hr) &&
                    left_identity != nullptr && left_identity == right_identity;
  if (left_identity != nullptr) left_identity->Release();
  if (right_identity != nullptr) right_identity->Release();
  return same;
}

inline bool ListenerFocusedContextIs(ITfThreadMgr* manager, ITfContext* expected) {
  if (manager == nullptr || expected == nullptr) return false;
  ITfDocumentMgr* document = nullptr;
  ITfContext* current = nullptr;
  const HRESULT focus_hr = manager->GetFocus(&document);
  const HRESULT top_hr = SUCCEEDED(focus_hr) && document != nullptr
                             ? document->GetTop(&current) : E_FAIL;
  const bool same = SUCCEEDED(top_hr) && ListenerSameComIdentity(current, expected);
  if (current != nullptr) current->Release();
  if (document != nullptr) document->Release();
  return same;
}
