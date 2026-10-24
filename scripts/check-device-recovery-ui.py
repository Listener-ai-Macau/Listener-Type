"""Production React/IPC-wrapper recovery ownership with controlled native results.
No actual GATT, recording, pairing, system settings, daily profile or clipboard.
"""
import argparse,json,re,datetime
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':False,'cases':[]}
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=b.new_page(viewport={'width':1280,'height':900});page.set_default_timeout(7000)
  page.add_init_script("""
if(!localStorage.getItem('ol.locale'))localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.rcalls=[];window.recoveries=[];window.cancelGate=null;window.deferCancel=false;window.failCancel=false;window.failSettings=false;
window.recoveryFixture=(cmd,args,mock)=>{
 window.rcalls.push({cmd,args});
 if(cmd==='cancel_dictation'&&window.failCancel)return Promise.reject('fixture_cancel_failed');
 if(cmd==='cancel_dictation'&&window.deferCancel)return new Promise(resolve=>window.cancelGate=resolve);
 if(cmd==='recover_embedded_ble_device')return new Promise((resolve,reject)=>window.recoveries.push({resolve,reject}));
 if(cmd==='open_system_settings'&&window.failSettings)return Promise.reject('fixture_settings_unavailable');
 return mock();
};
window.completeRecovery=(i,options={})=>window.recoveries[i].resolve({recovered:true,userActionRequired:false,openBluetoothSettings:false,message:'fixture connected',failure:null,runtime:{},firmware:{},...options});
""")
  def ipc(route):
   response=route.fetch();text=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert old in text
   body=text.replace(old,'if (!isTauri) {\n    return window.recoveryFixture(cmd,args,mock);\n  }',1)
   route.fulfill(response=response,body=body+'\nwindow.__recoveryUnderTest={recoverEmbeddedBleDevice,getPendingEmbeddedBleRecovery};\n')
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),ipc)
  page.goto(a.url,wait_until='networkidle');page.locator('button[title=Settings]').click();page.get_by_role('button',name='About',exact=True).click()
  control=page.locator('[data-device-recovery-control]');button=control.locator('button');status=control.locator('[role=status]')
  def calls(cmd):return page.evaluate('(cmd)=>window.rcalls.filter(x=>x.cmd===cmd).length',cmd)
  before={k:calls(k) for k in ['cancel_dictation','recover_embedded_ble_device','open_system_settings']}
  page.evaluate('window.deferCancel=true');button.click();expect(button).to_be_disabled();expect(status).to_contain_text('Checking')
  assert calls('cancel_dictation')==before['cancel_dictation']+1 and calls('recover_embedded_ble_device')==before['recover_embedded_ble_device']
  # Actual navigation unmounts and remounts About while cancellation is pending.
  page.get_by_role('button',name='Personalize',exact=True).click();page.get_by_role('button',name='About',exact=True).click();expect(button).to_be_disabled()
  assert calls('cancel_dictation')==before['cancel_dictation']+1
  page.evaluate('window.cancelGate();window.deferCancel=false');page.wait_for_function('window.recoveries.length===1')
  expect(button).to_be_disabled();assert calls('recover_embedded_ble_device')==before['recover_embedded_ble_device']+1
  r['cases'].append('Whole cancellation/native operation is owned across About unmount/remount; no second cancel or recovery')
  # Same actual exported wrapper used by Overview/pairing joins the operation.
  joined=page.evaluate("""()=>{const m=window.__recoveryUnderTest;const active=m.getPendingEmbeddedBleRecovery();return active===m.recoverEmbeddedBleDevice(1000)&&active===m.recoverEmbeddedBleDevice(4000,true)}""")
  assert joined and calls('recover_embedded_ble_device')==before['recover_embedded_ble_device']+1
  page.evaluate('window.completeRecovery(0)');expect(button).to_be_enabled();expect(status).to_have_text('BLE audio path is available. Device healthy.')
  assert calls('open_system_settings')==before['open_system_settings']
  r['cases'].append('Concurrent actual shared wrapper entries join one operation/result; successful reattached UI is visible without extra OS action')
  # A fresh action after completion is allowed; backend reports user action.
  button.click();page.wait_for_function('window.recoveries.length===2');expect(button).to_be_disabled()
  page.evaluate("window.completeRecovery(1,{recovered:false,userActionRequired:true,openBluetoothSettings:true,message:'Fixture: pair Listener in Bluetooth'})")
  expect(button).to_be_enabled();expect(status).to_have_text('Fixture: pair Listener in Bluetooth')
  assert calls('open_system_settings')==before['open_system_settings']+1
  r['cases'].append('New explicit attempt after completion works; unsuccessful result remains visible and opens requested system settings once')
  button.click();page.wait_for_function('window.recoveries.length===3');page.evaluate("window.recoveries[2].reject('fixture_native_recovery_failed')")
  expect(button).to_be_enabled();expect(status).to_be_visible()
  assert calls('open_system_settings')==before['open_system_settings']+2
  assert page.evaluate("!window.__recoveryUnderTest.getPendingEmbeddedBleRecovery()")
  r['cases'].append('Rejected native request releases shared operation, shows failure and retains one existing system-settings fallback')
  page.evaluate('window.failSettings=true');button.click();page.wait_for_function('window.recoveries.length===4');page.evaluate("window.completeRecovery(3,{openBluetoothSettings:true})")
  expect(button).to_be_enabled();expect(status).to_have_text('Could not open Bluetooth settings. Open your system’s Bluetooth settings manually.')
  r['cases'].append('System-settings failure is visible and never leaves the recovery button busy')
  page.evaluate('window.failSettings=false');button.click();page.wait_for_function('window.recoveries.length===5')
  os_before=calls('open_system_settings');page.get_by_role('button',name='Personalize',exact=True).click();page.evaluate("window.completeRecovery(4,{openBluetoothSettings:true})")
  page.wait_for_function("!window.__recoveryUnderTest.getPendingEmbeddedBleRecovery()")
  assert calls('open_system_settings')==os_before
  page.get_by_role('button',name='About',exact=True).click();expect(button).to_be_enabled()
  r['cases'].append('A closed About page cannot later steal focus with stale automatic OS-settings opening; ownership still releases')
  page.evaluate('window.failCancel=true');button.click();page.wait_for_function('window.recoveries.length===6')
  page.evaluate('window.completeRecovery(5)');expect(button).to_be_enabled();expect(status).to_have_text('BLE audio path is available. Device healthy.')
  r['cases'].append('Existing cancellation-error tolerance is preserved and still performs a single recovery')
  r['controlledCallsBeforeLocaleReload']=page.evaluate('window.rcalls.filter(x=>["cancel_dictation","recover_embedded_ble_device","open_system_settings"].includes(x.cmd))')
  # Success and pending copy use existing complete locale resources.
  for locale in ['zh-CN','zh-TW','ja','ko']:
   page.evaluate('(locale)=>localStorage.setItem("ol.locale",locale)',locale);page.reload(wait_until='networkidle')
   page.locator('button[title]').first.click();nav=page.locator('button').filter(has_text=re.compile({'zh-CN':'^关于$','zh-TW':'^關於$','ja':'^情報$','ko':'^정보$'}[locale]))
   nav.click();expect(page.locator('[data-device-recovery-control] button')).to_be_enabled()
   assert 'modal.about.' not in page.locator('[data-device-recovery-control]').inner_text()
  r['cases'].append('All five shipped locale buttons use existing UI strings; no hardcoded English controls')
  r['passed']=True
 finally:
  b.close();r['finishedAt']=datetime.datetime.now().astimezone().isoformat();a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(r,ensure_ascii=False))
