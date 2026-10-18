"""Replay production optional DATA_B discovery and bounded retry with fake WinRT edges.

No radio, product process, focus, key or clipboard operations are performed.
--before-root accepts frozen gatt_open.rs / ota_open.rs to preserve the counterexample.
"""
from pathlib import Path
import argparse
import hashlib
import json
import subprocess
import tempfile

repo = Path(__file__).resolve().parent.parent
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--source-root', type=Path, default=repo / 'src-tauri/src/embedded_ble/windows_ble')
p.add_argument('--before-root', type=Path)
p.add_argument('--output', type=Path)
a = p.parse_args()
root = a.before_root or a.source_root
prefix = 'before-' if a.before_root else ''
gatt = (root / f'{prefix}gatt_open.rs').read_text(encoding='utf8')
ota = (root / f'{prefix}ota_open.rs').read_text(encoding='utf8')

def function(text, name):
    start = text.index('fn ' + name + '(')
    end = text.find('\nfn ', start + 1)
    return text[start:end if end != -1 else None]

def lane(name):
    text = function(ota, name)
    start = text.index('    let data_b')
    end = text.index('    let status =', start)
    return text[start:end] + '    Ok(data_b)\n'

fixture = r'''
#![allow(dead_code, unused_variables, non_snake_case, non_upper_case_globals)]
extern crate self as log;
#[macro_export] macro_rules! info {($($arg:tt)*)=>{()}}
#[macro_export] macro_rules! warn {($($arg:tt)*)=>{()}}
use std::{cell::RefCell, collections::VecDeque, time::{Duration, Instant}};
type GUID = u128;
const LISTENER_OTA_V1_DATA_B_UUID: GUID = 0x13;
const BLE_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(50);
const AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS: [Duration; 2] = [Duration::ZERO; 2];
#[derive(Clone, Copy, Debug)] enum BluetoothCacheMode { Cached, Uncached }
#[derive(Clone, Copy, Debug, PartialEq)] struct GattCommunicationStatus(u8);
impl GattCommunicationStatus { const Success: Self = Self(0); }
#[derive(Clone, Copy, Debug)] struct GattCharacteristicProperties(u8);
impl GattCharacteristicProperties {
 const Write: Self = Self(1); const WriteWithoutResponse: Self = Self(2);
 fn contains(self, other: Self) -> bool { self.0 & other.0 != 0 }
}
#[derive(Clone, Debug)] struct GattCharacteristic { properties: Result<u8, String> }
impl GattCharacteristic {
 fn CharacteristicProperties(&self) -> Result<GattCharacteristicProperties, String> {
  self.properties.clone().map(GattCharacteristicProperties)
 }
}
#[derive(Clone)] struct Frame { status: u8, count: u32, wait_error: Option<&'static str>, properties: Result<u8, String> }
fn frame(status: u8, count: u32) -> Frame { Frame { status, count, wait_error: None, properties: Ok(2) } }
struct GattDeviceService { frames: RefCell<VecDeque<Frame>>, requests: RefCell<usize> }
impl GattDeviceService {
 fn new(frames: Vec<Frame>) -> Self { Self { frames: RefCell::new(frames.into()), requests: RefCell::new(0) } }
 fn GetCharacteristicsForUuidWithCacheModeAsync(&self, _: GUID, _: BluetoothCacheMode) -> Result<Operation, String> {
  *self.requests.borrow_mut() += 1;
  Ok(Operation(self.frames.borrow_mut().pop_front().expect("unexpected discovery")))
 }
}
struct Operation(Frame);
impl Operation {
 fn wait_ble_result(self, _: Duration, _: &str) -> Result<Discovery, String> {
  if let Some(e) = self.0.wait_error { Err(e.into()) } else { Ok(Discovery(self.0)) }
 }
}
struct Discovery(Frame);
impl Discovery {
 fn Status(&self) -> Result<GattCommunicationStatus, String> { Ok(GattCommunicationStatus(self.0.status)) }
 fn Characteristics(&self) -> Result<List, String> { Ok(List(self.0.clone())) }
}
struct List(Frame);
impl List {
 fn Size(&self) -> Result<u32, String> { Ok(self.0.count) }
 fn GetAt(&self, _: u32) -> Result<GattCharacteristic, String> { Ok(GattCharacteristic { properties: self.0.properties.clone() }) }
}
type PreparedListenerOtaV1Characteristics = Option<GattCharacteristic>;
fn remaining_ble_timeout(deadline: Instant, cap: Duration, label: &str) -> Result<Duration, String> {
 let left = deadline.checked_duration_since(Instant::now()).filter(|v| !v.is_zero())
  .ok_or_else(|| format!("{label} timed out"))?;
 Ok(left.min(cap))
}
'''
native = gatt[gatt.index('fn open_write_characteristic_from_service('):gatt.index('fn open_indicate_characteristic_from_service(')]
native += '\nfn open_listener_ota_v1_characteristics_from_service(service: &GattDeviceService, cache_mode: BluetoothCacheMode) -> Result<PreparedListenerOtaV1Characteristics, String> {\n' + lane('open_listener_ota_v1_characteristics_from_service') + '}\n'
native += '\nfn open_listener_ota_v1_characteristics_from_service_with_deadline(service: &GattDeviceService, cache_mode: BluetoothCacheMode, deadline: Instant) -> Result<PreparedListenerOtaV1Characteristics, String> {\n' + lane('open_listener_ota_v1_characteristics_from_service_with_deadline') + '}\n'
for name in ['open_listener_ota_v1_characteristics_from_service_with_retry', 'open_listener_ota_v1_characteristics_from_service_with_retry_deadline', 'is_transient_listener_ota_v1_discovery_error']:
    native += '\n' + function(ota, name)
tests = r'''
#[test] fn transient_disconnect_retries_instead_of_silent_single_lane() {
 let s = GattDeviceService::new(vec![frame(1, 0), frame(0, 1)]);
 assert!(open_listener_ota_v1_characteristics_from_service_with_retry(&s, BluetoothCacheMode::Cached).unwrap().is_some());
 assert_eq!(*s.requests.borrow(), 2);
}
#[test] fn deadline_path_recovers_transient_failure() {
 let s = GattDeviceService::new(vec![frame(3, 0), frame(0, 1)]);
 assert!(open_listener_ota_v1_characteristics_from_service_with_retry_deadline(&s, BluetoothCacheMode::Uncached, Instant::now()+Duration::from_secs(1)).unwrap().is_some());
 assert_eq!(*s.requests.borrow(), 2);
}
#[test] fn timed_out_optional_discovery_retries() {
 let mut f = frame(0, 1); f.wait_error = Some("characteristic discovery wait timed out");
 let s = GattDeviceService::new(vec![f, frame(0, 1)]);
 assert!(open_listener_ota_v1_characteristics_from_service_with_retry(&s, BluetoothCacheMode::Cached).unwrap().is_some());
}
#[test] fn persistent_failure_remains_bounded_and_visible() {
 let s = GattDeviceService::new(vec![frame(1, 0); 3]);
 assert!(open_listener_ota_v1_characteristics_from_service_with_retry(&s, BluetoothCacheMode::Cached).is_err());
 assert_eq!(*s.requests.borrow(), 3);
}
#[test] fn successful_absence_keeps_legacy_single_lane_compatibility() {
 let s = GattDeviceService::new(vec![frame(0, 0)]);
 assert!(open_listener_ota_v1_characteristics_from_service_with_retry(&s, BluetoothCacheMode::Cached).unwrap().is_none());
 assert_eq!(*s.requests.borrow(), 1);
}
#[test] fn nonwritable_characteristic_is_not_absence() {
 let mut f = frame(0, 1); f.properties = Ok(0);
 let s = GattDeviceService::new(vec![f]);
 assert!(open_listener_ota_v1_characteristics_from_service(&s, BluetoothCacheMode::Cached).unwrap_err().contains("not writable"));
}
#[test] fn expired_deadline_cannot_become_missing_hardware() {
 let s = GattDeviceService::new(vec![]);
 assert!(open_listener_ota_v1_characteristics_from_service_with_deadline(&s, BluetoothCacheMode::Cached, Instant::now()-Duration::from_millis(1)).is_err());
 assert_eq!(*s.requests.borrow(), 0);
}
#[test] fn required_characteristic_still_rejects_absence() {
 let s = GattDeviceService::new(vec![frame(0, 0)]);
 assert!(open_write_characteristic_from_service(&s, 1, "required", BluetoothCacheMode::Cached).unwrap_err().contains("not found"));
}
'''
source = fixture + native + tests
with tempfile.TemporaryDirectory(prefix='listener-optional-lane-') as tmp:
    out = a.output or Path(tmp)
    out.mkdir(parents=True, exist_ok=True)
    file = out / 'optional-lane-replay.rs'
    exe = out / 'optional-lane-replay.exe'
    file.write_text(source, encoding='utf8')
    build = subprocess.run([str(Path.home()/'.cargo/bin/rustc.exe'), '--edition=2021', '--test', str(file), '-o', str(exe)])
    result = subprocess.run([str(exe), '--test-threads=1']) if build.returncode == 0 else build
    (out/'proof.json').write_text(json.dumps({
        'sourceRoot': str(root), 'frozenBefore': bool(a.before_root),
        'sourceSha256': {name: hashlib.sha256((root/(prefix+name)).read_bytes()).hexdigest() for name in ['gatt_open.rs', 'ota_open.rs']},
        'productionGattFunctionsUnchanged': True, 'productionOptionalLaneStatementsUnchanged': True,
        'productionRetryFunctionsUnchanged': True, 'compileExit': build.returncode,
        'testExit': result.returncode, 'hardwareAccess': False,
        'limitation': 'Mock WinRT boundary replay, not a physical radio throughput result.'
    }, indent=2), encoding='utf8')
    raise SystemExit(result.returncode)
