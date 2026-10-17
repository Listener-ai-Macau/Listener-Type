"""Exercise the production runtime-cache function under overlapping startup/wake calls.

The model factory is replaced by a bounded slow factory. No model, profile,
device, network, installed process or editor is accessed.
"""
from pathlib import Path
import argparse,subprocess,tempfile,os

t=Path(__file__).resolve().parent.parent
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source',type=Path,default=t/'src-tauri/src/speaker_verification.rs')
args=parser.parse_args()
source=args.source.read_text(encoding='utf-8')
start=source.index('    fn ensure_runtime() -> Result<Arc<SpeakerRuntime>, String> {')
brace=source.index('{',start);depth=1;end=brace+1
while depth:
    if source[end]=='{':depth+=1
    if source[end]=='}':depth-=1
    end+=1
function=source[start:end]
fixture=r'''
use std::{path::{Path,PathBuf},sync::{Arc,Mutex,MutexGuard,LazyLock,Barrier},
    sync::atomic::{AtomicUsize,AtomicBool,Ordering},thread,time::Duration};
struct Lock<T>(Mutex<T>);
impl<T> Lock<T> { fn new(value:T)->Self {Self(Mutex::new(value))}
    fn lock(&self)->MutexGuard<'_,T> {self.0.lock().unwrap()} }
struct State {runtime:Option<Arc<SpeakerRuntime>>}
static STATE:LazyLock<Lock<State>>=LazyLock::new(||Lock::new(State{runtime:None}));
static RUNTIME_INIT_LOCK:LazyLock<Lock<()>>=LazyLock::new(||Lock::new(()));
static LOADS:AtomicUsize=AtomicUsize::new(0);
static FAIL_ONCE:AtomicBool=AtomicBool::new(false);
static LOADING:AtomicBool=AtomicBool::new(false);
const MODEL_URL:&str="offline";const MODEL_NAME:&str="offline";const MODEL_SHA256:&str="offline";
struct SpeakerRuntime {id:usize}
impl SpeakerRuntime {fn load(_: &Path)->Result<Self,String> {
    let id=LOADS.fetch_add(1,Ordering::SeqCst)+1;
    LOADING.store(true,Ordering::SeqCst);
    // Keep the first cache miss open while all callers reach production code.
    thread::sleep(Duration::from_millis(60));
    if FAIL_ONCE.swap(false,Ordering::SeqCst) {return Err("factory unavailable".into())}
    Ok(Self{id})
}}
fn ensure_runtime_assets()->Result<PathBuf,String> {Ok(PathBuf::from("offline"))}
fn download_verified(_: &str,_: &Path,_: &str)->Result<(),String> {Ok(())}
'''+function+r'''
fn main() {
    let mut failures=0;
    let start=Arc::new(Barrier::new(9));
    let callers=(0..8).map(|_|{let start=start.clone();thread::spawn(move||{
        start.wait();ensure_runtime().unwrap()
    })}).collect::<Vec<_>>();
    start.wait();
    while !LOADING.load(Ordering::SeqCst) {thread::yield_now()}
    // Loading must not hold the public profile/status mutex.
    if STATE.0.try_lock().is_err() {eprintln!("FAIL status/profile lock blocked by initialization");failures+=1}
    let loaded=callers.into_iter().map(|x|x.join().unwrap()).collect::<Vec<_>>();
    let count=LOADS.load(Ordering::SeqCst);
    if count!=1 {eprintln!("FAIL overlapping startup/wake callers created {count} runtimes");failures+=1}
    if loaded.iter().any(|x|!Arc::ptr_eq(x,&loaded[0])) {eprintln!("FAIL callers received different runtime owners");failures+=1}
    for _ in 0..8 {let ready=ensure_runtime().unwrap();assert!(Arc::ptr_eq(&ready,STATE.lock().runtime.as_ref().unwrap()));}
    assert_eq!(LOADS.load(Ordering::SeqCst),count);
    STATE.lock().runtime=None;LOADS.store(0,Ordering::SeqCst);FAIL_ONCE.store(true,Ordering::SeqCst);
    assert!(ensure_runtime().is_err());
    assert!(STATE.lock().runtime.is_none());
    let recovered=ensure_runtime().unwrap();assert_eq!(recovered.id,2);
    assert_eq!(LOADS.load(Ordering::SeqCst),2);
    println!("overlap_loads={count} status_responsive=true error_retry_loads=2 failures={failures}");
    if failures>0 {std::process::exit(1)}
}
'''
with tempfile.TemporaryDirectory(prefix='listener-runtime-init-') as folder:
    folder=Path(folder);(folder/'main.rs').write_text(fixture,encoding='utf-8')
    exe=folder/('test.exe' if os.name=='nt' else 'test')
    subprocess.run(['rustc','--edition=2021','-Awarnings',str(folder/'main.rs'),'-o',str(exe)],check=True,timeout=60)
    subprocess.run([str(exe)],check=True,timeout=10)
