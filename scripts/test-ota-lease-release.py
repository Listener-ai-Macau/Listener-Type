"""Execute production SYNC-release eligibility and protect its actual call site."""
from pathlib import Path
import subprocess,tempfile
R=Path(__file__).resolve().parents[1]
s=(R/'src-tauri/src/embedded_ble/windows_ble/mod.rs').read_text()
name='fn listener_ota_v1_sync_progress_confirms_prime('
start=s.index(name);opening=s.index('{',start);depth=0
for end in range(opening,len(s)):
    depth+=(s[end]=='{')-(s[end]=='}')
    if not depth:break
function=s[start:end+1]
transfer=(R/'src-tauri/src/embedded_ble/windows_ble/ota_transfer.rs').read_text()
converge=transfer[transfer.index('fn converge_active_link_before_begin'):transfer.index('fn handoff_new_generation_to_post_confirm')]
assert 'prime.close(' not in converge and 'status.active_link_confirmed()' in converge
read=s[s.index('    fn read_status(&mut self)'):]
assert 'listener_ota_v1_sync_progress_confirms_prime(' in read
assert 'prime.close("first_device_confirmed_sync_progress")' in read
constants=R/'third_party/denzic-platform/ota/host/rust/src/generated.rs'
fixture='''
#[test]fn release_requires_response_bearing_sync_and_valid_written_bytes(){
 use denzic_ota_core::*;
 assert!(listener_ota_v1_sync_progress_confirms_prime(true,STATE_RECEIVING,ERROR_NONE,28352,5194512));
 assert!(listener_ota_v1_sync_progress_confirms_prime(true,STATE_RECEIVING,ERROR_OFFSET_MISMATCH,443,5194512));
 assert!(!listener_ota_v1_sync_progress_confirms_prime(false,STATE_RECEIVING,ERROR_NONE,28352,5194512));
 for (state,error,written,expected) in [(STATE_IDLE,ERROR_NONE,443,5194512),(STATE_RECEIVING,ERROR_STORAGE_WRITE,443,5194512),(STATE_RECEIVING,ERROR_NONE,0,5194512),(STATE_RECEIVING,ERROR_NONE,443,0),(STATE_RECEIVING,ERROR_NONE,1001,1000)] {
  assert!(!listener_ota_v1_sync_progress_confirms_prime(true,state,error,written,expected));
 }
}
'''
with tempfile.TemporaryDirectory(prefix='listener-ota-lease-') as d:
    p=Path(d);(p/'check.rs').write_text('#![allow(dead_code)]\nmod denzic_ota_core { include!(r"'+str(constants)+'"); }\n'+function+fixture)
    subprocess.run(['rustc','--edition=2021','--test',str(p/'check.rs'),'-o',str(p/'check.exe')],check=True)
    subprocess.run([str(p/'check.exe')],check=True)
print('Production lease eligibility and pre-BEGIN/read-status ownership locations passed')
