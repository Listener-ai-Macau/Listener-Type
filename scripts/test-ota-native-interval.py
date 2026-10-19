"""Compile the actual native-budget predicate; retain normal prime ownership."""
from pathlib import Path
import subprocess,tempfile
r=Path(__file__).resolve().parents[1]
s=(r/'src-tauri/src/embedded_ble/windows_ble/mod.rs').read_text()
start=s.index('fn listener_ota_v1_native_rx_budget_ready(');opening=s.index('{',start);depth=0
for end in range(opening,len(s)):
    depth+=(s[end]=='{')-(s[end]=='}')
    if not depth:break
function=s[start:end+1]
fixture='''
#[test]fn actual_same_link_budget_requires_full_encrypted_2m_packet_time(){
 let ready=|octets:u16,time:u16,phy:u16|format!("ready=1;ota_rx_budget_v1=1;ota_rx_octets={octets};ota_rx_time_us={time};ota_rx_phy={phy}");
 for time in [1064,1590,2120] {assert!(listener_ota_v1_native_rx_budget_ready(ready(251,time,2).as_bytes()));}
 for (octets,time,phy) in [(251,965,2),(251,1063,2),(27,2120,2),(251,2120,1),(251,2120,3)] {
  assert!(!listener_ota_v1_native_rx_budget_ready(ready(octets,time,phy).as_bytes()));
 }
 for bad in [b"ready=1".as_slice(),b"ota_rx_budget_v1=1;ota_rx_octets=251;ota_rx_time_us=2120",b"ota_rx_budget_v1=1;ota_rx_octets=251;ota_rx_time_us=999999;ota_rx_phy=2",b"ota_rx_budget_v1=2;ota_rx_octets=251;ota_rx_time_us=2120;ota_rx_phy=2",b"ota_rx_budget_v1=1;ota_rx_octets=251;ota_rx_time_us=2120;ota_rx_phy=2;ota_rx_phy=1",b"\\xff"] {
  assert!(!listener_ota_v1_native_rx_budget_ready(bad));
 }
}
'''
with tempfile.TemporaryDirectory(prefix='listener-native-budget-') as d:
    p=Path(d);(p/'check.rs').write_text(function+fixture)
    subprocess.run(['rustc','--edition=2021','--test',str(p/'check.rs'),'-o',str(p/'check.exe')],check=True)
    subprocess.run([str(p/'check.exe')],check=True)
transfer=(r/'src-tauri/src/embedded_ble/windows_ble/ota_transfer.rs').read_text()
assert 'prime.is_none() && !native_trial' in transfer
assert 'status.active_link_confirmed() && native_budget_ready' in transfer
assert 'self.service.as_ref().ok_or("Native-interval trial requires the exact retained OTA service")' in transfer
print('Actual predicate: old965 budget rejected, full1064/1590/2120 accepted; normal prime and exact retained-service confirmation remain required.')
