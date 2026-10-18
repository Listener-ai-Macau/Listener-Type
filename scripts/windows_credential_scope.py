"""Keep Listener WinCred entries in the same Roaming scope as keyring-rs.

Windows can retain same-target Local(2) and Roaming(3) files. An unrelated
write selects a scope's older duplicate again, even preserving LastWritten.
Never use Local persistence for normal Listener credential writes.
"""
import ctypes
from ctypes import wintypes as W

KEYRING_PERSISTENCE = 3  # keyring 3.6.3 windows.rs: CRED_PERSIST_ENTERPRISE

class Credential(ctypes.Structure):
    _fields_ = [('Flags',W.DWORD),('Type',W.DWORD),('TargetName',W.LPWSTR),
                ('Comment',W.LPWSTR),('LastWritten',W.FILETIME),
                ('CredentialBlobSize',W.DWORD),('CredentialBlob',ctypes.POINTER(ctypes.c_ubyte)),
                ('Persist',W.DWORD),('AttributeCount',W.DWORD),('Attributes',ctypes.c_void_p),
                ('TargetAlias',W.LPWSTR),('UserName',W.LPWSTR)]

api=ctypes.WinDLL('advapi32',use_last_error=True)
api.CredReadW.argtypes=[W.LPCWSTR,W.DWORD,W.DWORD,ctypes.POINTER(ctypes.POINTER(Credential))]
api.CredReadW.restype=W.BOOL
api.CredWriteW.argtypes=[ctypes.POINTER(Credential),W.DWORD];api.CredWriteW.restype=W.BOOL
api.CredDeleteW.argtypes=[W.LPCWSTR,W.DWORD,W.DWORD];api.CredDeleteW.restype=W.BOOL
api.CredFree.argtypes=[ctypes.c_void_p]

def admit(target):
    if not (target.endswith('.com.listener.type') or target.startswith('com.listener.type.test.scope.')):
        raise ValueError('Only the Listener vault or unique scope fixtures are supported')

def read(target):
    admit(target);pointer=ctypes.POINTER(Credential)()
    if not api.CredReadW(target,1,0,ctypes.byref(pointer)):
        code=ctypes.get_last_error()
        if code==1168:return None
        raise OSError(code,'Read Listener protected credential failed')
    try:
        c=pointer.contents
        return {'blob':ctypes.string_at(c.CredentialBlob,c.CredentialBlobSize),
                'persist':c.Persist,'lastWrittenTicks':(c.LastWritten.dwHighDateTime<<32)|c.LastWritten.dwLowDateTime}
    finally:api.CredFree(pointer)

def _write(target,blob,persist):
    admit(target);buffer=(ctypes.c_ubyte*len(blob)).from_buffer_copy(blob)
    c=Credential();c.Type=1;c.TargetName=target;c.UserName=target
    c.Comment='Listener Type keyring-compatible protected configuration'
    c.CredentialBlobSize=len(blob);c.CredentialBlob=buffer;c.Persist=persist
    if not api.CredWriteW(ctypes.byref(c),0):
        raise OSError(ctypes.get_last_error(),'Write Listener protected credential failed')

def write(target,blob):
    """Normal configuration writes use only the application's scope."""
    _write(target,blob,KEYRING_PERSISTENCE)

def _delete_selected(target):
    admit(target)
    if not api.CredDeleteW(target,1,0):
        code=ctypes.get_last_error()
        if code!=1168:raise OSError(code,'Remove selected Listener duplicate failed')

def canonicalize(target,blob):
    """Remove a pre-existing Local shadow while retaining a durable good copy.

    Called only in an idle maintenance window. The Roaming copy is complete
    before deleting the Local file, and a failure attempts to restore the
    selected entry. The caller must retain a protected recovery generation.
    """
    write(target,blob)
    _write(target,blob,2)  # Select/update the Local shadow before deleting it.
    try:
        _delete_selected(target)
    finally:
        write(target,blob)
    actual=read(target)
    if actual is None or actual['persist']!=KEYRING_PERSISTENCE or actual['blob']!=blob:
        raise RuntimeError('Canonical protected entry verification failed')

def delete_fixture(target):
    if not target.startswith('com.listener.type.test.scope.'):
        raise ValueError('Fixture cleanup cannot delete product credentials')
    for persist in (2,3):
        _write(target,b'fixture-cleanup',persist);_delete_selected(target)
