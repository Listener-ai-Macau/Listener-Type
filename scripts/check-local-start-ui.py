"""Reproduce unprepared activation, then check cancellation/failure preserve dictation."""
import argparse, datetime, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
p=argparse.ArgumentParser(); p.add_argument('--url',required=True); p.add_argument('--report',type=Path,required=True); p.add_argument('--before',action='store_true'); p.add_argument('--screenshot',type=Path); a=p.parse_args(); checks=[]
def check(name,ok):
 checks.append({'check':name,'passed':bool(ok)}); assert ok,name
fixture_js="""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
window.__localLog=[]; window.__prepareOutcome='pending'; window.__loadedAlias=null; window.__activationFails=false;
window.__localFixture=(cmd,args,mock)=>{
 window.__localLog.push({cmd,args});
 if(cmd==='foundry_local_asr_prepare') {
   if(window.__prepareOutcome==='preview') return mock();
   if(window.__prepareOutcome==='reject') return Promise.reject(new Error('Sample download blocked'));
   if(window.__prepareOutcome==='pending') return new Promise((resolve,reject)=>{window.__releasePrepare=()=>{window.__loadedAlias=args.modelAlias;resolve('fixture-'+args.modelAlias)};window.__cancelPrepare=()=>reject(new Error('Preparation cancelled'));});
   window.__loadedAlias=args.modelAlias; return Promise.resolve('fixture-'+args.modelAlias);
 }
 if(cmd==='foundry_local_asr_cancel_prepare') {window.__cancelPrepare?.(); return Promise.resolve();}
 if(cmd==='set_active_asr_provider' && window.__activationFails) return Promise.reject(new Error('Sample activation rejected'));
 if(cmd==='foundry_local_asr_status') {const value=mock();return {...value,runtimeReady:!!window.__loadedAlias,loadedModelId:window.__loadedAlias,activeModel:window.__loadedAlias??value.activeModel};}
 return mock();
};
"""
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=b.new_page(viewport={'width':1280,'height':1050});page.set_default_timeout(6000);page.add_init_script(fixture_js); patches=[]
  def fixture(route):
   r=route.fetch(); s=r.text()
   if a.before:
    s,n=re.subn(r'(async function (\w+)\((\w+),(\w+),(\w+)\)\{if\(!\w+\)return )\5\(\);',lambda m:m[1]+'window.__localFixture('+m[3]+','+m[4]+','+m[5]+');',s)
   else:
    old='if (!isTauri) {\n    return mock();\n  }'; n=s.count(old);s=s.replace(old,'if (!isTauri) {\n    return window.__localFixture(cmd,args,mock);\n  }',1)
   patches.extend([True]*n);route.fulfill(response=r,body=s)
  page.route('**/assets/*.js' if a.before else re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  page.goto(a.url,wait_until='networkidle');check('Controlled backend applies only inside this isolated browser',bool(patches))
  page.get_by_role('button',name='Microphone and input source',exact=True).click();page.get_by_role('button',name='Local recognition',exact=True).click()
  if a.before:
   page.get_by_role('button',name='Set default / Enable Windows local ASR',exact=True).click()
   page.wait_for_function("window.__localLog.some(x=>x.cmd==='set_active_asr_provider')")
   log=page.evaluate('window.__localLog')
   check('Pre-fix: provider is activated without preparing or loading the model',not any(x['cmd']=='foundry_local_asr_prepare' for x in log))
  else:
   button=page.get_by_role('button',name=re.compile('Prepare and use local recognition|^Preparing…$|^Enabling…$|^Cancelling…$'))
   expect(page.get_by_role('button',name=re.compile('Whisper Tiny.*151'))).to_be_visible()
   page.get_by_role('button',name=re.compile('Whisper Tiny.*151')).click()
   check('Smaller download is an explicit choice without changing the active provider',not any(x['cmd'] in ['set_active_asr_provider','foundry_local_asr_set_model','foundry_local_asr_prepare'] for x in page.evaluate('window.__localLog')))
   button.click();page.wait_for_function("window.__localLog.some(x=>x.cmd==='foundry_local_asr_prepare')")
   expect(button).to_be_disabled()
   check('Pending preparation does not change provider or saved model',not any(x['cmd'] in ['set_active_asr_provider','foundry_local_asr_set_model'] for x in page.evaluate('window.__localLog')))
   page.get_by_role('button',name='Cancel prepare',exact=True).click();expect(page.get_by_text('Preparation cancelled',exact=True)).to_be_visible()
   check('Cancelled preparation keeps the existing provider and model',not any(x['cmd'] in ['set_active_asr_provider','foundry_local_asr_set_model'] for x in page.evaluate('window.__localLog')))
   page.evaluate("window.__prepareOutcome='reject';window.__localLog=[]")
   button.click();expect(page.get_by_text('Sample download blocked',exact=True)).to_be_visible()
   check('Download failure keeps the existing provider and model',not any(x['cmd'] in ['set_active_asr_provider','foundry_local_asr_set_model'] for x in page.evaluate('window.__localLog')))
   page.evaluate("window.__prepareOutcome='resolve';window.__activationFails=true;window.__localLog=[]")
   button.click();expect(page.get_by_text('Sample activation rejected',exact=True)).to_be_visible()
   check('Activation rejection does not claim a saved local selection',not any(x['cmd']=='set_settings' and x.get('args',{}).get('prefs',{}).get('activeAsrProvider')=='foundry-local-whisper' for x in page.evaluate('window.__localLog')))
   page.evaluate('window.__activationFails=false;window.__localLog=[]');button.click();expect(page.get_by_text('In use',exact=True)).to_be_visible()
   log=page.evaluate('window.__localLog');commands=[x['cmd'] for x in log]
   activation=next(x for x in log if x['cmd']=='set_active_asr_provider')
   check('Successful activation commits the explicitly selected model after preparation',commands.index('foundry_local_asr_prepare')<commands.index('set_active_asr_provider') and activation['args']['modelAlias']=='whisper-tiny')
   page.evaluate("window.__prepareOutcome='preview';window.__localLog=[]");button.click();expect(page.get_by_role('alert')).to_contain_text('This browser preview cannot download or run recognition')
   check('Ordinary browser preview never claims to download or prepare a real model',not any(x['cmd']=='set_active_asr_provider' for x in page.evaluate('window.__localLog')))
   if a.screenshot:
    page.wait_for_timeout(400);page.screenshot(path=str(a.screenshot),full_page=True)
 finally:b.close()
r={'at':datetime.datetime.now().astimezone().isoformat(),'url':a.url,'before':a.before,'scope':'Headless browser with controlled IPC failures; no models, native Type, cloud, credentials, clipboard or hardware touched.','checks':checks,'passed':all(x['passed'] for x in checks)}
a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2),encoding='utf-8');print(json.dumps(r,ensure_ascii=False))
