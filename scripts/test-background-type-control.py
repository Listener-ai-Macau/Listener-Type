"""Compile actual capture-loop background-control methods with fake WinRT I/O.

The old/current recovery callback bodies are extracted verbatim. No BLE, ASR,
editor or clipboard is opened. --output retains the generated bounded replay.
"""
from pathlib import Path
import argparse
import subprocess
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
t = Path(__file__).resolve().parents[1]
m = (t / 'src-tauri/src/embedded_ble/windows_ble/mod.rs').read_text(encoding='utf-8')
c = (t / 'src-tauri/src/embedded_ble/windows_ble/capture_events.rs').read_text(encoding='utf-8')
old = subprocess.check_output(['git', '-C', str(t), 'show',
    '9fa367c3a93f6ae51270e3167e98caa0e5351c6f:src-tauri/src/embedded_ble/windows_ble/capture_events.rs']).decode('utf-8')

def item(text, start):
    pos = text.index(start)
    brace = text.index('{', pos)
    level = 1
    end = brace + 1
    while level:
        level += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[pos:end]

fixture = r'''
use std::{sync::{Arc,Mutex,atomic::{AtomicUsize,AtomicBool,Ordering}},time::{Instant,Duration},marker::PhantomData};
extern crate self as log;
#[macro_export] macro_rules! info {($($t:tt)*)=>{{}}}
#[macro_export] macro_rules! warn {($($t:tt)*)=>{{}}}
mod windows {pub mod Foundation {#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub enum AsyncStatus{Started,Completed,Error,Canceled,Other}}}
use windows::Foundation::AsyncStatus;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]enum GattCommunicationStatus{Success,Unreachable}
struct FakeOp{status:AtomicUsize,result:GattCommunicationStatus,canceled:AtomicBool,closed:AtomicBool,reads:AtomicUsize}
struct IAsyncOperation<T>{state:Arc<FakeOp>,_marker:PhantomData<T>}
impl IAsyncOperation<GattCommunicationStatus>{
 fn Status(&self)->Result<AsyncStatus,String>{Ok(match self.state.status.load(Ordering::SeqCst){0=>AsyncStatus::Started,1=>AsyncStatus::Completed,2=>AsyncStatus::Error,_=>AsyncStatus::Canceled})}
 fn GetResults(&self)->Result<GattCommunicationStatus,String>{assert_eq!(self.Status()?,AsyncStatus::Completed,"must never wait on an unfinished operation");self.state.reads.fetch_add(1,Ordering::SeqCst);Ok(self.state.result)}
 fn Cancel(&self)->Result<(),String>{self.state.canceled.store(true,Ordering::SeqCst);Ok(())}
 fn Close(&self)->Result<(),String>{self.state.closed.store(true,Ordering::SeqCst);Ok(())}
}
static OPS:Mutex<Vec<Arc<FakeOp>>>=Mutex::new(Vec::new());
static SYNC_WAIT_MS:AtomicUsize=AtomicUsize::new(0);
const TYPE_HEARTBEAT_WRITE_TIMEOUT:Duration=Duration::from_millis(750);
fn type_heartbeat_write_option(_: &(),_: &str){}
fn start_gatt_write_with_option_async(_: &(),_: &[u8],_: (),_: &str)->Result<IAsyncOperation<GattCommunicationStatus>,String>{let state=Arc::new(FakeOp{status:AtomicUsize::new(0),result:GattCommunicationStatus::Success,canceled:AtomicBool::new(false),closed:AtomicBool::new(false),reads:AtomicUsize::new(0)});OPS.lock().unwrap().push(state.clone());Ok(IAsyncOperation{state,_marker:PhantomData})}
struct Target{control:Option<()>}
struct NotifyCleanup{capture_id:u64,target:Target,pending_type_heartbeat:Option<PendingTypeHeartbeat>,type_heartbeat_preempted:bool,lossless_recovery_pending:bool,urgent:bool}
impl NotifyCleanup{
 fn new()->Self{Self{capture_id:1,target:Target{control:Some(())},pending_type_heartbeat:None,type_heartbeat_preempted:false,lossless_recovery_pending:false,urgent:false}}
 fn has_pending_audio_control(&self)->bool{self.urgent}
 // Hardware boundary for the extracted old callback only: its synchronous
 // write occupies the capture caller for this existing750ms timeout.
 fn write_type_heartbeat(&self,_:&[u8],_:&str)->Result<(),String>{SYNC_WAIT_MS.fetch_add(750,Ordering::SeqCst);Err("test pending WinRT write".into())}
}
fn collector_has_active_recoverable_session(collector:&bool)->bool{*collector}
'''
body = '#[derive(Debug,PartialEq,Eq)]\n' + item(m, 'enum TypeHeartbeatPollDecision')
body += item(m, 'fn type_heartbeat_poll_decision(')
body += '#[derive(Clone,Copy,Debug,Eq,PartialEq)]\n' + item(m, 'enum BackgroundTypeControlKind')
for start in ['impl BackgroundTypeControlKind', 'struct PendingTypeHeartbeat',
              'impl PendingTypeHeartbeat', 'impl Drop for PendingTypeHeartbeat']:
    body += item(m, start)
body += '\nimpl NotifyCleanup {\n'
for start in ['fn begin_periodic_type_heartbeat(', 'fn begin_background_type_control(',
              'fn poll_periodic_type_heartbeat(', 'fn preempt_background_type_control(',
              'fn schedule_lossless_capability_recovery(']:
    body += item(m, start)
body += '\n}\n'
for name, text in [('old_recovery_callback', old), ('current_recovery_callback', c)]:
    branch = item(text, 'if should_restore_lossless_after_heartbeat(')
    body += '\nfn ' + name + '(cleanup:&mut NotifyCleanup,consecutive_type_heartbeat_failures:u32,collector:bool){let capture_id=1;\n' + branch + '\n}\n'
body += item(m, 'fn should_restore_lossless_after_heartbeat(')
timeout_start = m.index('const TYPE_HEARTBEAT_WRITE_TIMEOUT:')
timeout_end = m.index(';', timeout_start) + 1
fixture = fixture.replace(
    'const TYPE_HEARTBEAT_WRITE_TIMEOUT:Duration=Duration::from_millis(750);',
    m[timeout_start:timeout_end])
tests = r'''
#[test]fn old_recovery_wait_is_removed_from_actual_callback(){let mut c=NotifyCleanup::new();SYNC_WAIT_MS.store(0,Ordering::SeqCst);old_recovery_callback(&mut c,1,false);assert_eq!(SYNC_WAIT_MS.load(Ordering::SeqCst),750);SYNC_WAIT_MS.store(0,Ordering::SeqCst);current_recovery_callback(&mut c,1,false);assert_eq!(SYNC_WAIT_MS.load(Ordering::SeqCst),0);assert!(c.lossless_recovery_pending);}
#[test]fn pending_capability_never_reads_unfinished_results(){let mut c=NotifyCleanup::new();c.lossless_recovery_pending=true;c.schedule_lossless_capability_recovery(false);let op=c.pending_type_heartbeat.as_ref().unwrap().operation.state.clone();assert!(c.poll_periodic_type_heartbeat().is_none());assert_eq!(op.reads.load(Ordering::SeqCst),0);c.preempt_background_type_control();assert!(op.canceled.load(Ordering::SeqCst)&&op.closed.load(Ordering::SeqCst));assert!(c.lossless_recovery_pending);assert!(!c.type_heartbeat_preempted);}
#[test]fn optional_write_waits_for_capture_and_urgent_ownership(){let mut c=NotifyCleanup::new();c.lossless_recovery_pending=true;c.schedule_lossless_capability_recovery(true);assert!(c.pending_type_heartbeat.is_none());c.urgent=true;c.schedule_lossless_capability_recovery(false);assert!(c.pending_type_heartbeat.is_none());c.urgent=false;c.schedule_lossless_capability_recovery(false);assert_eq!(c.pending_type_heartbeat.as_ref().unwrap().kind,BackgroundTypeControlKind::LosslessRecovery);}
#[test]fn optional_timeout_is_a_capability_outcome_not_a_heartbeat_failure(){let mut c=NotifyCleanup::new();c.lossless_recovery_pending=true;c.schedule_lossless_capability_recovery(false);c.pending_type_heartbeat.as_mut().unwrap().started_at=Instant::now()-Duration::from_millis(751);let(kind,result)=c.poll_periodic_type_heartbeat().unwrap();assert_eq!(kind,BackgroundTypeControlKind::LosslessRecovery);assert!(result.unwrap_err().contains("timed out"));assert!(c.pending_type_heartbeat.is_none());assert!(!c.type_heartbeat_preempted);}
#[test]fn heartbeat_preemption_still_requires_its_original_reschedule(){let mut c=NotifyCleanup::new();c.begin_periodic_type_heartbeat().unwrap();let op=c.pending_type_heartbeat.as_ref().unwrap().operation.state.clone();c.preempt_background_type_control();assert!(c.type_heartbeat_preempted);assert!(!c.lossless_recovery_pending);assert!(op.canceled.load(Ordering::SeqCst)&&op.closed.load(Ordering::SeqCst));}
#[test]fn optional_completion_cannot_replace_a_pending_heartbeat(){let mut c=NotifyCleanup::new();c.begin_periodic_type_heartbeat().unwrap();c.lossless_recovery_pending=true;c.schedule_lossless_capability_recovery(false);assert_eq!(c.pending_type_heartbeat.as_ref().unwrap().kind,BackgroundTypeControlKind::Heartbeat);c.pending_type_heartbeat.as_ref().unwrap().operation.state.status.store(1,Ordering::SeqCst);let(kind,result)=c.poll_periodic_type_heartbeat().unwrap();assert_eq!(kind,BackgroundTypeControlKind::Heartbeat);assert!(result.is_ok());c.schedule_lossless_capability_recovery(false);c.pending_type_heartbeat.as_ref().unwrap().operation.state.status.store(1,Ordering::SeqCst);let(kind,result)=c.poll_periodic_type_heartbeat().unwrap();assert_eq!(kind,BackgroundTypeControlKind::LosslessRecovery);assert!(result.is_ok());}
'''
a.output.mkdir(parents=True, exist_ok=True)
source = a.output / 'background-control-replay.rs'
exe = a.output / 'background-control-replay.exe'
source.write_text(fixture + body + tests, encoding='utf-8')
subprocess.run(['rustc', '--edition=2021', '--test', str(source), '-o', str(exe)], check=True)
raise SystemExit(subprocess.run([str(exe), '--test-threads=1']).returncode)
