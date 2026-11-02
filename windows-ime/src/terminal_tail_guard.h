#pragma once

#include <msctf.h>
#include <limits>
#include <string>
#include <vector>

enum class ListenerTerminalTail {
  kMatches,
  kChanged,
  kUnavailable,
};

// Call only under a TSF edit lock. Do not log document text. This checks the
// current caret and exact suffix together, rather than relying on an earlier
// insertion receipt after the editor may have been submitted or changed.
inline ListenerTerminalTail ListenerInspectTerminalTail(
    ITfContext* context, TfEditCookie cookie, const std::wstring& expected) {
  constexpr size_t kMaxTailUnits = 32768;
  if (context == nullptr || expected.empty() || expected.size() > kMaxTailUnits ||
      expected.size() > static_cast<size_t>((std::numeric_limits<LONG>::max)())) {
    return ListenerTerminalTail::kUnavailable;
  }
  TF_SELECTION selection = {};
  ULONG fetched = 0;
  HRESULT hr = context->GetSelection(cookie, TF_DEFAULT_SELECTION, 1, &selection, &fetched);
  if (FAILED(hr) || fetched != 1 || selection.range == nullptr) {
    if (selection.range != nullptr) selection.range->Release();
    return ListenerTerminalTail::kUnavailable;
  }

  BOOL empty = FALSE;
  hr = selection.range->IsEmpty(cookie, &empty);
  if (FAILED(hr)) { selection.range->Release(); return ListenerTerminalTail::kUnavailable; }
  if (!empty || selection.style.fInterimChar) {
    selection.range->Release(); return ListenerTerminalTail::kChanged;
  }
  ITfRange* end = nullptr;
  hr = context->GetEnd(cookie, &end);
  LONG comparison = 1;
  if (SUCCEEDED(hr) && end != nullptr) {
    hr = selection.range->CompareEnd(cookie, end, TF_ANCHOR_END, &comparison);
  } else if (SUCCEEDED(hr)) {
    hr = E_UNEXPECTED;
  }
  if (end != nullptr) end->Release();
  if (FAILED(hr)) { selection.range->Release(); return ListenerTerminalTail::kUnavailable; }
  if (comparison != 0) { selection.range->Release(); return ListenerTerminalTail::kChanged; }

  ITfRange* prefix = nullptr;
  hr = selection.range->Clone(&prefix);
  selection.range->Release();
  if (FAILED(hr) || prefix == nullptr) {
    if (prefix != nullptr) prefix->Release();
    return ListenerTerminalTail::kUnavailable;
  }
  LONG shifted = 0;
  hr = prefix->ShiftStart(cookie, -static_cast<LONG>(expected.size()), &shifted, nullptr);
  if (FAILED(hr)) { prefix->Release(); return ListenerTerminalTail::kUnavailable; }
  if (shifted != -static_cast<LONG>(expected.size())) {
    prefix->Release(); return ListenerTerminalTail::kChanged;
  }
  std::vector<wchar_t> actual(expected.size() + 1);
  ULONG count = 0;
  hr = prefix->GetText(cookie, 0, actual.data(), static_cast<ULONG>(actual.size()), &count);
  prefix->Release();
  if (FAILED(hr)) return ListenerTerminalTail::kUnavailable;
  return std::wstring(actual.data(), count) == expected
             ? ListenerTerminalTail::kMatches : ListenerTerminalTail::kChanged;
}
