#pragma once

#include <msctf.h>
#include <windows.h>

inline constexpr CLSID CLSID_ListenerTypeTextService = {
    0xe6d16c6c,
    0x2975,
    0x4a5c,
    {0xbb, 0xbb, 0x67, 0xa3, 0xc9, 0x96, 0x67, 0x67},
};

inline constexpr GUID GUID_ListenerTypeProfile = {
    0x19f96d43,
    0xa5eb,
    0x46c9,
    {0x8a, 0x73, 0x9f, 0xca, 0x5a, 0x06, 0x30, 0xc8},
};

inline constexpr wchar_t kListenerTypeImeName[] = L"Listener Type Voice Input";
inline constexpr LANGID kListenerTypeLangId = 0x0804;
