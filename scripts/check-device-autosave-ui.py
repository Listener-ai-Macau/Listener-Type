"""Actual React autosave with controlled USB/BLE board IPC boundaries.
Ordinary headless UI, no hardware, cloud, Owner desktop or device qualification.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
r={'startedAt':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':False}
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge');page=browser.new_page(viewport={'width':1500,'height':1100});page.set_default_timeout(7000)
 try:
  page.add_init_script(r'''
localStorage.setItem('ol.locale','zh-CN');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
window.__calls=[];window.__boards={};window.__failNext=null;window.__holdRead=null;window.__held=[];
const devices=[{id:'usb:A',transport:'usb',label:'USB A'},{id:'ble:A',transport:'ble',label:'BLE A'},{id:'usb:B',transport:'usb',label:'USB B'}];
const mapping=()=>({action:'disabled',appPage:'settingsDevice',externalAppPath:'',pasteTemplate:'',shortcut:null});
const group=()=>Object.fromEntries(['key1','key2','key3','key4','knob'].map(k=>[k,mapping()]));
window.__fixture=async(cmd,args,mock)=>{
 window.__calls.push({cmd,args});
 if(cmd==='list_settings_devices')return structuredClone(devices);
 if(cmd==='list_installed_applications')return [{name:'Fixture App',path:'C:\\Fixture\\app.exe'}];
 if(!['get_selected_device_settings','set_selected_device_settings','set_selected_device_key_profile'].includes(cmd))return mock();
 const id=args.device.id,uid=id.split(':')[1];let b=window.__boards[uid];
 if(!b){const s=mock().snapshot;b=window.__boards[uid]={revision:10,keysRevision:4,lease:0,snapshot:{...s,source:'firmware',connected:true,writeSupported:true,bleName:uid==='A'?'Alpha':'Bravo',statusLedBrightnessPercent:uid==='A'?21:73,keyLedBrightnessPercent:37,knobLedBrightnessPercent:49,edgeLedBrightnessPercent:56,pluggedLowPowerIdleMinutes:5,batteryLowPowerIdleMinutes:7,pluggedLowPowerEnabled:true,voiceAutoStartEnabled:true,voiceAutoStopEnabled:true,pluggedAutoShutdownMs:0,batteryAutoShutdownMs:19*60000,knobRotationAction:'system_volume'},profile:{schema:1,deviceCustomKeys:group(),deviceCustomKeyDoubleClicks:{...group(),key4:{...mapping(),pasteTemplate:uid==='B'?'Retained Bravo note':''}},deviceCustomKeyLongPresses:group(),hid:Array.from({length:15},()=>[1,0,0])}};}
 if(cmd==='get_selected_device_settings'){
  if(window.__holdRead===id)await new Promise(resolve=>window.__held.push(resolve));
  b.lease++;
 }else{
  if(window.__failNext===cmd){window.__failNext=null;throw new Error('controlled settings transport failure');}
  if(args.revision!==b.revision || args.readToken!==id+':'+b.lease)throw new Error('stale read lease');
  if(cmd==='set_selected_device_key_profile'){
   if(args.profileRevision!==b.keysRevision)throw new Error('stale key revision');
   b.profile=structuredClone(args.profile);b.keysRevision++;
  }else{
   const {batteryAutoShutdownMinutes,pluggedAutoShutdownMinutes,...q}=args.request;
   b.snapshot={...b.snapshot,...q,batteryAutoShutdownMs:batteryAutoShutdownMinutes*60000,pluggedAutoShutdownMs:pluggedAutoShutdownMinutes*60000};
  }
  b.revision++;
 }
 return {device:structuredClone(args.device),revision:b.revision,readToken:id+':'+b.lease,snapshot:structuredClone(b.snapshot),keys:{deviceUid:uid,revision:b.keysRevision,profile:structuredClone(b.profile)},keysError:null};
};
''')
  def fixture(route):
   response=route.fetch();source=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert source.count(old)==1
   route.fulfill(response=response,body=source.replace(old,'if (!isTauri) {\n    return window.__fixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  page.goto(a.url,wait_until='networkidle');page.evaluate('window.__holdRead="usb:A"');page.locator('button[title]').first.click();page.locator('.ol-settings-nav button').first.click()
  core=page.locator('.ol-device-settings-card');keys=page.locator('fieldset').filter(has=page.get_by_role('combobox',name='选择设备键动作').first)
  page.wait_for_function('window.__held.length===1')
  pending_number=core.locator('input[type=number]').nth(3);pending_number.fill('42');pending_number.press('Tab')
  page.wait_for_function('window.__held.length===2')
  assert page.evaluate('window.__calls.filter(x=>x.cmd.startsWith("set_selected_device")).length')==0
  page.evaluate('window.__holdRead=null;window.__held.splice(0).forEach(r=>r())');expect(core).to_contain_text('已保存到设备')
  assert page.evaluate('window.__boards.A.snapshot.bleName')=='Alpha';assert page.evaluate('window.__boards.A.snapshot.statusLedBrightnessPercent')==42
  pending_number.fill('21');pending_number.press('Tab');expect(core).to_contain_text('已保存到设备');page.evaluate('window.__calls=[]');r['explicitEditBeforeInitialReadCompletesStillAutosaves']=True
  expect(core.locator('input[placeholder=listener]')).to_have_value('Alpha')
  expect(keys.get_by_role('combobox',name='选择设备键动作')).to_have_count(13)
  writes=lambda:page.evaluate('window.__calls.filter(x=>x.cmd.startsWith("set_selected_device"))')
  def choose(control,label):
   control.scroll_into_view_if_needed();control.focus();page.mouse.move(5,5);page.wait_for_timeout(180);control.press('ArrowDown')
   opts=page.get_by_role('listbox').get_by_role('option');expect(opts.first).to_be_attached()
   values=opts.evaluate_all('(es)=>es.map(e=>({text:e.innerText.trim(),selected:e.getAttribute("aria-selected")==="true"}))')
   current=next(i for i,x in enumerate(values) if x['selected']);target=next(i for i,x in enumerate(values) if x['text']==label)
   for _ in range((target-current)%len(values)):control.press('ArrowDown')
   control.press('Enter');expect(page.get_by_role('listbox')).to_have_count(0);expect(control).to_contain_text(label)
  def quiet():page.wait_for_timeout(300)
  quiet();assert writes()==[];r['initialReadDoesNotWrite']=True
  status=core.locator('input[type=number]').nth(3);status.fill('42');quiet();assert writes()==[]
  status.press('Tab');expect(core).to_contain_text('已保存到设备');w=writes();assert len(w)==1 and w[0]['args']['request']['bleName']=='Alpha';assert w[0]['args']['request']['batteryAutoShutdownMinutes']==19
  r['numericBlurAutosavesOnlyExplicitChange']=True
  choose(core.get_by_role('combobox',name='连接方式'),'蓝牙无线');expect(core.locator('input[placeholder=listener]')).to_have_value('Alpha');expect(status).to_have_value('42');quiet();assert len(writes())==1
  r['usbToBluetoothReadDoesNotOverwriteBoard']=True
  k=keys.get_by_role('combobox',name='选择设备键动作').nth(7);choose(k,'复制 Ctrl+C');expect(keys).to_contain_text('已保存到设备');w=writes();assert len(w)==2 and w[-1]['args']['device']['id']=='ble:A';assert w[-1]['args']['profile']['deviceCustomKeyDoubleClicks']['key4']['action']=='copyShortcut';assert w[-1]['args']['knobRotationAction'] is None
  r['keyActionAutosavesOverSelectedBluetooth']=True
  page.evaluate('window.__failNext="set_selected_device_key_profile"');choose(k,'粘贴 Ctrl+V');expect(keys).to_contain_text('保存失败，修改已保留');expect(k).to_contain_text('粘贴 Ctrl+V');n=len(writes());quiet();assert len(writes())==n
  keys.get_by_role('button',name='重试保存',exact=True).click();expect(keys).to_contain_text('已保存到设备');assert page.evaluate('window.__boards.A.profile.deviceCustomKeyDoubleClicks.key4.action')=='pasteShortcut';r['failureRetainsEditAndExplicitRetryRecovers']=True
  choose(core.get_by_role('combobox',name='连接方式'),'USB 有线');expect(core.locator('input[placeholder=listener]')).to_have_value('Alpha');quiet();n=len(writes());page.evaluate('window.__holdRead="usb:B"');choose(core.get_by_role('combobox',name='设置设备'),'USB B');page.wait_for_function('window.__held.length===1');assert len(writes())==n
  choose(k,'复制 Ctrl+C');page.wait_for_function('window.__held.length===2');page.evaluate('window.__holdRead=null;window.__held.splice(0).forEach(r=>r())');expect(keys).to_contain_text('已保存到设备')
  assert page.evaluate('window.__boards.B.profile.deviceCustomKeyDoubleClicks.key4.pasteTemplate')=='Retained Bravo note';r['keyActionBeforeReadPreservesUneditedStoredFields']=True
  choose(k,'无');expect(keys).to_contain_text('已保存到设备');n=len(writes());expect(core.locator('input[placeholder=listener]')).to_have_value('Bravo');expect(status).to_have_value('73');quiet();assert len(writes())==n
  assert page.evaluate('window.__boards.A.snapshot.bleName')=='Alpha';r['deviceSwitchNeverPushesPreviousNameOrDefaults']=True
  name=core.locator('input[placeholder=listener]');name.fill('Bravo-Renamed');quiet();assert len(writes())==n;name.press('Tab');expect(core).to_contain_text('已保存到设备');assert writes()[-1]['args']['device']['id']=='usb:B';assert page.evaluate('window.__boards.A.snapshot.bleName')=='Alpha'
  r['nameCommitsOnceAfterEditingToExactUsbBoard']=True
  # A delayed read for a real explicit edit must stay pinned to its original target.
  choose(core.get_by_role('combobox',name='设置设备'),'USB A');expect(name).to_have_value('Alpha');page.evaluate('window.__holdRead="usb:A"')
  choose(k,'撤销 Ctrl+Z');page.wait_for_function('window.__held.length===1');choose(core.get_by_role('combobox',name='设置设备'),'USB B');expect(name).to_have_value('Bravo-Renamed')
  page.evaluate('window.__holdRead=null;window.__held.splice(0).forEach(r=>r())');page.wait_for_function('window.__boards.A.profile.deviceCustomKeyDoubleClicks.key4.action==="undoShortcut"')
  assert page.evaluate('window.__boards.B.profile.deviceCustomKeyDoubleClicks.key4.action')=='disabled';r['inFlightSaveCannotRetargetToAnotherDevice']=True
  n=len(writes());long_key=keys.get_by_role('combobox',name='选择设备键动作').nth(11);choose(long_key,'输入固定文字');quiet();assert len(writes())==n
  stack=long_key.locator('xpath=ancestor::div[contains(@class,"ol-device-key-control-stack")]');template=stack.locator('input');template.fill('Customer saved template');quiet();assert len(writes())==n
  template.press('Tab');expect(keys).to_contain_text('已保存到设备');assert page.evaluate('window.__boards.B.profile.deviceCustomKeyLongPresses.key4.pasteTemplate')=='Customer saved template';r['incompleteTemplateWaitsAndTextBlurAutosaves']=True
  n=len(writes());app_key=keys.get_by_role('combobox',name='选择设备键动作').nth(10);choose(app_key,'打开指定应用');quiet()
  app_stack=app_key.locator('xpath=ancestor::div[contains(@class,"ol-device-key-control-stack")]')
  choose(app_stack.get_by_role('combobox').nth(1),'Fixture App');expect(keys).to_contain_text('已保存到设备');assert page.evaluate('window.__boards.B.profile.deviceCustomKeyLongPresses.key3.externalAppPath')=='C:\\Fixture\\app.exe';r['installedApplicationChoiceAutosaves']=True
  r['writes']=writes();r['passed']=True
 except Exception as e:
  r['error']=str(e);page.screenshot(path=str(a.report.with_suffix('.png')),full_page=True)
 finally:browser.close()
r['finishedAt']=datetime.datetime.now().astimezone().isoformat();a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps({k:v for k,v in r.items() if k!='writes'},ensure_ascii=False));raise SystemExit(0 if r['passed'] else 1)

