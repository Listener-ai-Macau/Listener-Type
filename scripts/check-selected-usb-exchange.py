"""Actual selected USB Rust exchange with a deterministic Read/Write boundary.
No real COM port, app, Bluetooth or user configuration is accessed.
"""
import argparse,json,re,subprocess,tempfile,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser();p.add_argument('--source',type=Path,default=ROOT/'src-tauri/src/embedded_ble/windows_ble/settings_target.rs');p.add_argument('--expect-old-drain',action='store_true');p.add_argument('--output',type=Path);a=p.parse_args()
target=a.source.read_text(encoding='utf-8');shared=(ROOT/'src-tauri/src/embedded_ble/windows_ble/mod.rs').read_text(encoding='utf-8')
def function(name):
 m=re.search(r'^fn '+name+r'\([^;]*?\)\s*(?:->[^\{]+)?\{',shared,re.M);assert m,name
 start=m.start();pos=shared.index('{',m.start());depth=1;end=pos+1
 while depth:depth+=(shared[end]=='{')-(shared[end]=='}');end+=1
 return shared[start:end]+'\n'
actual=target[target.index('struct SelectedUsbSettings {'):target.index('impl SettingsIo for SelectedUsbSettings')]
constants='\n'.join(re.search(r'^const '+n+r'\b[^;]+;',shared,re.M)[0] for n in ['DEVICE_SETTINGS_SERIAL_DRAIN_MAX_DURATION','DEVICE_SETTINGS_SERIAL_DRAIN_QUIET_DURATION','DEVICE_SETTINGS_SERIAL_READ_CHUNK_BYTES'])
prefix=r'''
use std::{io::{self,Read,Write},time::{Duration,Instant},sync::{Arc,Mutex},collections::VecDeque};
mod log { macro_rules! fixture_log {($($t:tt)*)=>{}} pub(crate) use fixture_log as debug; pub(crate) use fixture_log as warn; }
mod serialport { pub trait SerialPort:std::io::Read+std::io::Write+Send {} impl<T:std::io::Read+std::io::Write+Send> SerialPort for T {} }
#[derive(Default)] struct Facts {quiet_reads:usize,writes:usize,fail_on_write:Option<usize>}
struct Port {facts:Arc<Mutex<Facts>>,pending:VecDeque<Vec<u8>>,fail_next:bool}
impl Read for Port {fn read(&mut self,b:&mut[u8])->io::Result<usize>{
 let mut f=self.facts.lock().unwrap();if f.fail_on_write==Some(f.writes){f.fail_on_write=None;return Err(io::Error::new(io::ErrorKind::BrokenPipe,"injected disconnect"))}drop(f);
 if let Some(mut p)=self.pending.pop_front(){let n=b.len().min(p.len());b[..n].copy_from_slice(&p[..n]);p.drain(..n);if !p.is_empty(){self.pending.push_front(p)}return Ok(n)}
 self.facts.lock().unwrap().quiet_reads+=1;std::thread::sleep(Duration::from_millis(180));Err(io::Error::new(io::ErrorKind::TimedOut,"quiet boundary"))
}}
impl Write for Port {fn write(&mut self,b:&[u8])->io::Result<usize>{
 let mut f=self.facts.lock().unwrap();f.writes+=1;let n=f.writes;drop(f);
 // Split the actual acknowledgement across transport reads.
 if b.starts_with(b"~DEVICE:BAD"){self.pending.push_back(b"~DEVICE:ERROR key=TEST reason=ESP_FAIL\n".to_vec())}
 else {self.pending.push_back(b"ambient log\n~DEVICE:SETT".to_vec());self.pending.push_back(format!("INGS result=OK proof={n}\n").into_bytes())}Ok(b.len())
}fn flush(&mut self)->io::Result<()>{Ok(())}}
fn main(){
 let facts=Arc::new(Mutex::new(Facts::default()));
 let port=Port{facts:Arc::clone(&facts),pending:VecDeque::from([b"~DEVICE:SETTINGS result=OK proof=stale\n".to_vec()]),fail_next:false};
 let mut io=SelectedUsbSettings{port:Box::new(port),FIELDS};
 let start=Instant::now();let count=COUNT;
 for n in 1..=count{let got=io.exchange("DEVICE:KEY D 00000001 0000 00").unwrap();assert!(got.ends_with(&format!("proof={n}")),"initial stale or partial ACK used: {got}")}
 let quiet=facts.lock().unwrap().quiet_reads;let elapsed=start.elapsed().as_millis();
 facts.lock().unwrap().fail_on_write=Some(count+1);
 assert!(io.exchange("DEVICE:KEY D 00000001 0000 00").unwrap_err().contains("injected disconnect"));
 let got=io.exchange("DEVICE:SETTINGS").unwrap();assert!(got.ends_with(&format!("proof={}",count+2)),"stale ACK after error used: {got}");
 assert!(io.exchange("DEVICE:BAD").unwrap_err().contains("~DEVICE:ERROR"));
 let got=io.exchange("DEVICE:SETTINGS").unwrap();assert!(got.ends_with(&format!("proof={}",count+4)));
 let f=facts.lock().unwrap();let recovery=f.quiet_reads>=quiet+2;
 println!("{{\"writes\":{},\"quietWaits\":{},\"elapsedMs\":{},\"allFreshSplitAcknowledgements\":true,\"errorsResynchronizeWithoutRetry\":{},\"passed\":{}}}",count,quiet,elapsed,recovery,quiet==1&&recovery);
}
'''
fields='synchronized:false,' if 'synchronized:' in actual else ''
fixture=prefix[:prefix.index('fn main()')]+constants+'\n'+function('drain_serial_input_until_quiet')+function('complete_device_settings_ok_line')+actual+prefix[prefix.index('fn main()'):]
fixture=fixture.replace('FIELDS',fields).replace('COUNT','3' if a.expect_old_drain else '300')
build=Path(tempfile.mkdtemp(prefix='listener-usb-exchange-'));(build/'actual.rs').write_text(fixture,encoding='utf-8')
result=subprocess.run(['rustc','--edition=2021',str(build/'actual.rs'),'-o',str(build/'actual.exe')],capture_output=True,text=True);assert result.returncode==0,result.stderr
out=subprocess.run([str(build/'actual.exe')],capture_output=True,text=True,timeout=12);assert out.returncode==0,out.stderr
observed=json.loads(out.stdout);r={'source':str(a.source),'actualMethods':'SelectedUsbSettings with original drain and complete ACK parser','scope':__doc__,'build':str(build),'observed':observed,'expectedOldFailure':a.expect_old_drain,'passed':not observed['passed'] if a.expect_old_drain else observed['passed']}
if a.output:a.output.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(r,ensure_ascii=False,indent=2));raise SystemExit(0 if r['passed'] else 1)
