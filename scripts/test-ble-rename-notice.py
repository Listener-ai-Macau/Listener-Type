"""Replay the actual notify-ready decision with the saved rename reason.

Only lock storage, logging and unrelated radio classifiers are fixtures. There
is no BLE connection, editor input, clipboard access or installed-app control.
"""
from pathlib import Path
import argparse,subprocess,tempfile
root=Path(__file__).resolve().parent.parent
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source',type=Path,default=root/'src-tauri/src/coordinator/embedded_ble_runtime.rs')
args=parser.parse_args()
source=args.source.read_text(encoding='utf-8')
def function(name):
    start=source.index('fn '+name+'(');end=source.index('{',start)+1;depth=1
    while depth:
        depth+=(source[end]=='{')-(source[end]=='}');end+=1
    return source[start:end]
code=r'''
#![allow(dead_code,unused_variables)]
extern crate self as log;
#[macro_export] macro_rules! info {($($x:tt)*)=>{{let _=format_args!($($x)*);}}}
use std::sync::{Arc,Mutex,MutexGuard};
struct Lock<T>(Mutex<T>);impl<T> Lock<T> {fn lock(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()}}
#[derive(Clone,Debug)] enum EmbeddedBleWakeRecoveryStatus {Reconnecting,Ready}
#[derive(Clone,Debug)] enum EmbeddedBleNotifySubscriptionState {Opening,Lost,Failed,Subscribed}
struct Wake {recent_disconnect_reason:Option<String>,notify_subscription_state:EmbeddedBleNotifySubscriptionState,
status:EmbeddedBleWakeRecoveryStatus,usb_powered:Option<bool>,battery_percent:Option<u8>,reconnect_attempts:u32,
consecutive_reconnect_failures:u32,user_guidance:String,last_ready_at:Option<String>}
struct Inner {embedded_ble_wake_recovery:Lock<Wake>}
mod embedded_ble {#[derive(Debug)] pub struct Failure {pub kind:u8,pub automatic_recovery:bool}
pub fn classify_ble_failure(_: &str)->Failure{Failure{kind:0,automatic_recovery:false}}}
mod startup_evidence {pub fn record_background_notify_ready(){}}
fn clear_embedded_ble_type_pairasync_startup_guard(_: &Arc<Inner>,_: &str){}
fn is_embedded_ble_low_power_idle_candidate(_: &str)->bool{false}
fn is_embedded_ble_link_loss_error(_: &str)->bool{false}
fn embedded_ble_log_preview(s:&str)->String{s.to_string()}
fn now_rfc3339()->String{"fixture".into()}
'''+function('should_emit_embedded_ble_recovered_capsule_for_reason')+'\n'+function('record_embedded_ble_notify_ready')+r'''
fn inner(reason:&str,attempts:u32)->Arc<Inner>{Arc::new(Inner{embedded_ble_wake_recovery:Lock(Mutex::new(Wake{
recent_disconnect_reason:Some(reason.into()),notify_subscription_state:EmbeddedBleNotifySubscriptionState::Opening,
status:EmbeddedBleWakeRecoveryStatus::Reconnecting,usb_powered:Some(true),battery_percent:None,reconnect_attempts:attempts,
consecutive_reconnect_failures:1,user_guidance:String::new(),last_ready_at:None}))})}
fn main(){
 let renamed=inner("BLE name apply handoff",7);
 assert!(record_embedded_ble_notify_ready(&renamed),"saved rename completed but notification suppressed");
 assert!(!record_embedded_ble_notify_ready(&renamed),"duplicate ready repeats rename notification");
 for reason in ["refresh","shutdown","test","test cleanup","BLE name Windows cache refresh follow-up","当前已有听写会话在运行，暂不能提交嵌入式音频"] {
  assert!(!record_embedded_ble_notify_ready(&inner(reason,7)),"unrelated refresh claimed recovery: {reason}");
 }
 println!("PASS: saved rename notify-ready emits once; duplicate/refresh/shutdown/test/unconfirmed-followup/ownership-conflict stay silent");
}
'''
with tempfile.TemporaryDirectory(prefix='listener-rename-notice-',ignore_cleanup_errors=True) as directory:
    path=Path(directory);(path/'test.rs').write_text(code,encoding='utf-8')
    subprocess.run([str(Path.home()/'.cargo/bin/rustc.exe'),'--edition=2021',str(path/'test.rs'),'-o',str(path/'test.exe')],check=True)
    subprocess.run([str(path/'test.exe')],check=True)
