"""Execute the production packing selector at ATT/manifest capacity boundaries."""
from pathlib import Path
import hashlib, subprocess, tempfile

root=Path(__file__).resolve().parents[1]
source=(root/'src-tauri/src/embedded_ble/windows_ble/mod.rs').read_text(encoding='utf-8')
start=source.index('fn listener_ota_v1_packet_payload(')
opening=source.index('{',start);depth=0;end=None
for i in range(opening,len(source)):
    if source[i]=='{':depth+=1
    elif source[i]=='}':
        depth-=1
        if not depth:end=i+1;break
assert end
function=source[start:end]
constants='\n'.join(line for line in source.splitlines() if line.startswith('const LISTENER_OTA_V1_') and any(x in line.split(':')[0] for x in ['PROTOCOL_MAX_CHUNK_PAYLOAD_BYTES','CHUNK_PAYLOAD_BYTES','LL_PAYLOAD_BYTES','L2CAP_ATT_OFFSET_BYTES','PACKET_PROFILE_ENV']))
fixture='''
const ATT_WRITE_HEADER_BYTES:usize=3;
#[test] fn capacities_and_legacy_fallback(){
    assert_eq!(listener_ota_v1_packet_payload(500,500,None).unwrap(),443);
    assert_eq!(listener_ota_v1_packet_payload(500,20,None).unwrap(),20);
    assert_eq!(listener_ota_v1_packet_payload(500,500,Some("aligned251")).unwrap(),491);
    assert_eq!(listener_ota_v1_packet_payload(500,244,Some("aligned251")).unwrap(),240);
    assert!(listener_ota_v1_packet_payload(500,239,Some("aligned251")).is_err());
    for cap in 240..=500 {
        let data=listener_ota_v1_packet_payload(500,cap,Some("aligned251")).unwrap();
        assert!(data<=cap && data<=500 && data>0);
        assert_eq!((data+4+3+4)%251,0);
    }
    assert!(listener_ota_v1_packet_payload(0,500,None).is_err());
    assert!(listener_ota_v1_packet_payload(500,500,Some("typo")).is_err());
}
'''
with tempfile.TemporaryDirectory(prefix='listener-ota-packing-') as d:
    p=Path(d);(p/'check.rs').write_text(constants+'\n'+function+'\n'+fixture)
    subprocess.run(['rustc','--edition=2021','--test',str(p/'check.rs'),'-o',str(p/'check.exe')],check=True)
    subprocess.run([str(p/'check.exe')],check=True)
print('Production selector SHA256='+hashlib.sha256(function.encode()).hexdigest())
