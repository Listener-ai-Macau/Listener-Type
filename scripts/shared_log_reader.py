"""Read a live log without preventing Windows rotation.

The returned stream still refers to the same file after a rename. Tailers must
compare file identity with the current path and reopen after rotation.
"""
import os


def open_shared_log(path, *, encoding="utf-8", errors="replace"):
    if os.name != "nt":
        return open(path, encoding=encoding, errors=errors)

    import ctypes
    import msvcrt
    from ctypes import wintypes

    api = ctypes.WinDLL("kernel32", use_last_error=True)
    create = api.CreateFileW
    create.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                       ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD,
                       wintypes.HANDLE]
    create.restype = wintypes.HANDLE
    # GENERIC_READ, FILE_SHARE_READ | WRITE | DELETE, OPEN_EXISTING.
    handle = create(os.path.abspath(path), 0x80000000, 7, None, 3, 0x80, None)
    if handle == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        fd = msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
    except BaseException:
        close = api.CloseHandle
        close.argtypes = [wintypes.HANDLE]
        close.restype = wintypes.BOOL
        close(handle)
        raise
    try:
        return os.fdopen(fd, "r", encoding=encoding, errors=errors)
    except BaseException:
        os.close(fd)
        raise
