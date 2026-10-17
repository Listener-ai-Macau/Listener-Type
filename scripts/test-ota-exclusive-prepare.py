"""Replay actual OTA prepare/transfer ownership functions with fake WinRT I/O.

No device, installed process, network, recording, editor or clipboard is touched.
The production functions are extracted unchanged; radio results are not simulated.
"""
from pathlib import Path
import argparse,subprocess,tempfile,os

repo=Path(__file__).resolve().parent.parent
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--source-root',type=Path,default=repo/'src-tauri/src/embedded_ble/windows_ble')
p.add_argument('--output',type=Path)
a=p.parse_args()
mod=(a.source_root/'mod.rs').read_text(encoding='utf8')
transfer=(a.source_root/'ota_transfer.rs').read_text(encoding='utf8')
structs=mod[mod.index('enum PreparedListenerOtaV1TransferOwnership {'):mod.index('impl PreparedListenerOtaV1Transfer {')]
implementation=mod[mod.index('impl PreparedListenerOtaV1Transfer {'):mod.index('/// Host/link flakiness that should reopen')]
constructors=transfer[transfer.index('pub(super) fn prepare_listener_ota_v1_transfer()'):transfer.index('pub(super) fn is_ota_finish_reboot_handoff_error')]
classifier=mod[mod.index('pub(super) fn is_retryable_listener_ota_v1_transfer_error'):mod.index('fn listener_ota_v1_retired_sync_session')]
signal=''
if 'fn transfer_listener_ota_v1_signaled(' in transfer:
 signal=transfer[transfer.index('pub fn transfer_listener_ota_v1_after_active_link_hint_staged('):transfer.index('fn listener_ota_v1_window_chunks')]
fixture=r"""
#![allow(dead_code,unused_variables,unused_imports)]
extern crate self as log;
#[macro_export] macro_rules! info {($($arg:tt)*)=>{()}}
#[macro_export] macro_rules! warn {($($arg:tt)*)=>{()}}
#[macro_export] macro_rules! error {($($arg:tt)*)=>{()}}
use std::{sync::{Mutex,atomic::{AtomicBool,AtomicUsize,Ordering},mpsc},time::{Instant,Duration},collections::VecDeque};
static EVENTS:Mutex<Vec<String>>=Mutex::new(Vec::new());
static FAILURES:Mutex<VecDeque<&'static str>>=Mutex::new(VecDeque::new());
static CAPTURE:AtomicBool=AtomicBool::new(false);
static PROCESS:AtomicBool=AtomicBool::new(false);
static PREPARE:AtomicBool=AtomicBool::new(false);
static NEXT:AtomicUsize=AtomicUsize::new(1);
static BAD_CONVERGENCE:AtomicUsize=AtomicUsize::new(0);
fn note(s:impl Into<String>){EVENTS.lock().unwrap().push(s.into());}
fn reset(){assert!(!CAPTURE.load(Ordering::SeqCst));assert!(!PROCESS.load(Ordering::SeqCst));assert!(!PREPARE.load(Ordering::SeqCst));EVENTS.lock().unwrap().clear();FAILURES.lock().unwrap().clear();NEXT.store(1,Ordering::SeqCst);BAD_CONVERGENCE.store(0,Ordering::SeqCst);}
fn values(prefix:&str)->Vec<String>{EVENTS.lock().unwrap().iter().filter_map(|v|v.strip_prefix(prefix).map(str::to_owned)).collect()}
fn all_released(){assert!(!CAPTURE.load(Ordering::SeqCst));assert!(!PROCESS.load(Ordering::SeqCst));assert!(!PREPARE.load(Ordering::SeqCst));}
struct BleOtaPreparationGuard;
fn acquire_ble_ota_preparation_mutex(_: &str)->Result<BleOtaPreparationGuard,String>{assert!(!PREPARE.swap(true,Ordering::SeqCst));note("prepare_owned");Ok(BleOtaPreparationGuard)}
impl Drop for BleOtaPreparationGuard{fn drop(&mut self){PREPARE.store(false,Ordering::SeqCst);}}
struct BleOtaProcessGuard;
fn acquire_ble_ota_process_mutex(_: &str)->Result<BleOtaProcessGuard,String>{assert!(!PROCESS.swap(true,Ordering::SeqCst));note("process_owned");Ok(BleOtaProcessGuard)}
impl Drop for BleOtaProcessGuard{fn drop(&mut self){PROCESS.store(false,Ordering::SeqCst);}}
struct BleCaptureGuard;
impl BleCaptureGuard{fn enter(_:Option<Duration>)->Result<Self,String>{assert!(PROCESS.load(Ordering::SeqCst));assert!(!CAPTURE.swap(true,Ordering::SeqCst));note("capture_owned");Ok(Self)}fn session_id(&self)->u64{1}}
impl Drop for BleCaptureGuard{fn drop(&mut self){CAPTURE.store(false,Ordering::SeqCst);}}
struct BleFreshGattGuard;
impl BleFreshGattGuard{fn enter(_: &str)->Result<Self,String>{Ok(Self)}}
mod embedded_ble{#[derive(Debug)]pub struct FirmwareOtaTransferStats;pub struct FirmwareOtaDeviceSnapshot{pub detail:Option<String>}}
struct OpenListenerOtaV1Target{id:usize,bluetooth_address:Option<u64>}
impl Drop for OpenListenerOtaV1Target{fn drop(&mut self){note(format!("drop:{}",self.id));}}
fn opened()->OpenListenerOtaV1Target{assert!(PREPARE.load(Ordering::SeqCst));let id=NEXT.fetch_add(1,Ordering::SeqCst);note(format!("open:{id}"));note(format!("open_under_capture:{}",CAPTURE.load(Ordering::SeqCst)));OpenListenerOtaV1Target{id,bluetooth_address:Some(0x1234)}}
fn open_listener_ota_v1_target_after_active_link_handoff()->Result<OpenListenerOtaV1Target,String>{Ok(opened())}
fn open_listener_ota_v1_target_after_active_link_handoff_with_deadline(_:Instant)->Result<OpenListenerOtaV1Target,String>{Ok(opened())}
fn request_listener_ota_v1_active_link(_:Option<u64>)->Result<(),String>{Ok(())}
fn listener_ota_v1_gatt_probe_snapshot_from_target(_: &OpenListenerOtaV1Target)->embedded_ble::FirmwareOtaDeviceSnapshot{embedded_ble::FirmwareOtaDeviceSnapshot{detail:None}}
fn listener_ota_v1_transfer_ready_snapshot_from_target(t:&OpenListenerOtaV1Target)->embedded_ble::FirmwareOtaDeviceSnapshot{listener_ota_v1_gatt_probe_snapshot_from_target(t)}
struct OtaPreBulkMaintainHandoff{session:Option<()>}
impl OtaPreBulkMaintainHandoff{fn take_from(_: &mut OpenListenerOtaV1Target)->Self{note("preserve_old_maintain");Self{session:None}}fn transfer_to_fresh_target(&mut self,_:&OpenListenerOtaV1Target,_:usize){}}
fn request_concurrent_ble_power_optimized(_:Option<u64>){note("peer_policy");}
impl OpenListenerOtaV1Target{fn converge_active_link_before_begin(&mut self,_:u64)->Result<(),String>{assert!(CAPTURE.load(Ordering::SeqCst)&&PROCESS.load(Ordering::SeqCst));note(format!("converge:{}",self.id));if self.id==BAD_CONVERGENCE.load(Ordering::SeqCst){Err("active-link unconfirmed".into())}else{Ok(())}}fn handoff_new_generation_to_post_confirm(&mut self,_:u64){note("retain_confirmed_generation");}}
struct OtaRebootGenerationBoundary{connected:bool,att_ready:bool}
struct OtaRebootDisconnectObserver;
impl OtaRebootDisconnectObserver{fn arm(_:&OpenListenerOtaV1Target,_:u64)->Option<Self>{note("arm_before_begin");Some(Self)}fn wait_for_new_generation(&self,_:u64)->OtaRebootGenerationBoundary{OtaRebootGenerationBoundary{connected:true,att_ready:true}}}
fn transfer_denzic_ota_v1_to_target(t:&OpenListenerOtaV1Target,_:u64,_:&[u8],_:usize,_:Option<&dyn Fn(usize,usize)>)->Result<embedded_ble::FirmwareOtaTransferStats,String>{assert!(CAPTURE.load(Ordering::SeqCst)&&PROCESS.load(Ordering::SeqCst));note(format!("begin:{}",t.id));if let Some(e)=FAILURES.lock().unwrap().pop_front(){Err(e.into())}else{Ok(embedded_ble::FirmwareOtaTransferStats)}}
"""
tests=r"""
#[test]fn exclusively_prepared_target_is_used_without_a_second_open(){reset();let p=prepare_listener_ota_v1_transfer_impl(false,true,Some(Duration::from_secs(6))).unwrap();p.transfer("sha",b"image",500,None).unwrap();assert_eq!(values("open:"),vec!["1"]);assert_eq!(values("begin:"),vec!["1"]);assert_eq!(values("open_under_capture:"),vec!["true"]);all_released();}
#[test]fn legacy_staged_target_keeps_old_secure_reopen_protection(){reset();let p=prepare_listener_ota_v1_transfer_impl(false,false,Some(Duration::from_secs(6))).unwrap();assert!(!CAPTURE.load(Ordering::SeqCst));p.transfer("sha",b"image",500,None).unwrap();assert_eq!(values("open:"),vec!["1","2"]);assert_eq!(values("begin:"),vec!["2"]);assert_eq!(values("open_under_capture:"),vec!["false","true"]);all_released();}
#[test]fn authentication_failure_still_reopens_under_the_same_owner(){reset();FAILURES.lock().unwrap().push_back("protocol_error=14");let p=prepare_listener_ota_v1_transfer_impl(false,true,Some(Duration::from_secs(6))).unwrap();p.transfer("sha",b"image",500,None).unwrap();assert_eq!(values("open:"),vec!["1","2"]);assert_eq!(values("begin:"),vec!["1","2"]);all_released();}
#[test]fn cancellation_does_not_restart_an_image(){reset();FAILURES.lock().unwrap().push_back("OTA sync failed: OTA device session ended: canceled");let p=prepare_listener_ota_v1_transfer_impl(false,true,Some(Duration::from_secs(6))).unwrap();assert!(p.transfer("sha",b"image",500,None).is_err());assert_eq!(values("begin:").len(),1);all_released();}
#[test]fn unconfirmed_active_link_cannot_reach_begin(){reset();BAD_CONVERGENCE.store(1,Ordering::SeqCst);let p=prepare_listener_ota_v1_transfer_impl(false,true,Some(Duration::from_secs(6))).unwrap();p.transfer("sha",b"image",500,None).unwrap();assert_eq!(values("converge:"),vec!["1","2"]);assert_eq!(values("begin:"),vec!["2"]);all_released();}
"""
signal_tests=r"""
#[test]fn released_listener_entry_owns_capture_before_opening(){reset();let(tx,_rx)=mpsc::sync_channel(1);let(start_tx,start_rx)=mpsc::sync_channel(1);start_tx.send(()).unwrap();transfer_listener_ota_v1_after_listener_release_staged(tx,start_rx,Duration::from_secs(6),"sha",b"image",500,None).unwrap();assert_eq!(values("open:"),vec!["1"]);assert_eq!(values("open_under_capture:"),vec!["true"]);all_released();}
#[test]fn lost_ready_receiver_drops_prepared_target_and_both_transfer_guards(){reset();let(tx,rx)=mpsc::sync_channel(1);drop(rx);let(_start_tx,start_rx)=mpsc::sync_channel(1);assert!(transfer_listener_ota_v1_after_listener_release_staged(tx,start_rx,Duration::from_secs(6),"sha",b"image",500,None).is_err());assert!(values("begin:").is_empty());assert_eq!(values("drop:"),vec!["1"]);all_released();}
#[test]fn canceled_start_signal_does_not_begin(){reset();let(tx,_rx)=mpsc::sync_channel(1);let(start_tx,start_rx)=mpsc::sync_channel(1);drop(start_tx);assert!(transfer_listener_ota_v1_after_listener_release_staged(tx,start_rx,Duration::from_secs(6),"sha",b"image",500,None).is_err());assert!(values("begin:").is_empty());all_released();}
"""
source=fixture+'\nmod production {use super::*;\n'+structs+implementation+constructors+classifier+signal+tests+(signal_tests if signal else '')+'\n}\n'
if a.output:a.output.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix='listener-ota-owner-')as folder:
 root=a.output or Path(folder);file=root/'ownership-replay.rs';exe=root/'ownership-replay.exe';file.write_text(source,encoding='utf8')
 rustc=Path.home()/'.cargo/bin/rustc.exe';subprocess.run([str(rustc),'--edition=2021','--test',str(file),'-o',str(exe)],check=True)
 result=subprocess.run([str(exe),'--test-threads=1']);raise SystemExit(result.returncode)
