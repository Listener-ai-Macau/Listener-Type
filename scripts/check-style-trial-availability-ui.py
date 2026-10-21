"""Exercise pre-trial service status with local read-only receipts, no real keys or services."""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',type=Path);a=p.parse_args();checks=[]
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=browser.new_page(viewport={'width':1180,'height':1000});page.set_default_timeout(6000)
  page.add_init_script("localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');window.__trial={configured:false,available:true,fail:false,reads:0,writes:0,hold:false,pending:[],rawNeeds:false};")
  def fixture(route):
   response=route.fetch();s=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1
   new="""if(!isTauri){const f=window.__trial;if(cmd==='get_style_text_trial_readiness'){f.reads++;if(f.fail)throw new Error('isolated-read-failed');const s={available:f.available,requiresTextService:args.stylePackId!=='builtin.raw'||f.rawNeeds,textServiceConfigured:f.configured};if(f.hold)return new Promise(resolve=>f.pending.push(()=>resolve(s)));return s;}if(cmd.startsWith('set_')||cmd.startsWith('try_style'))f.writes++;return mock();}"""
   route.fulfill(response=response,body=s.replace(old,new,1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  page.goto(a.url,wait_until='networkidle');page.get_by_role('button',name='Style',exact=True).click()
  raw=page.locator('[data-style-availability="builtin.raw"]');light=page.locator('[data-style-availability="builtin.light"]')
  expect(raw).to_have_text('Original text · no text service needed');expect(light).to_have_text('Text service required · not configured')
  initial_writes=page.evaluate('window.__trial.writes')
  assert page.locator('dialog[open]').count()==0;checks.append('Requirements and missing setup visible before opening any trial')
  page.evaluate("window.__trial.configured=true;window.dispatchEvent(new Event('listener-type:credentials-status-changed'))")
  expect(light).to_have_text('Text service configured');checks.append('Successful configuration invalidation updates visible cards')
  page.evaluate("window.__trial.hold=true;window.dispatchEvent(new Event('listener-type:credentials-status-changed'))")
  page.wait_for_function('window.__trial.pending.length>=4')
  page.evaluate("window.__trial.hold=false;window.__trial.configured=false;window.dispatchEvent(new Event('listener-type:credentials-status-changed'))")
  expect(light).to_have_text('Text service required · not configured');page.evaluate('window.__trial.pending.splice(0).forEach(done=>done())');page.wait_for_timeout(100)
  expect(light).to_have_text('Text service required · not configured');checks.append('Late configured receipt cannot replace newer missing configuration')
  page.evaluate("window.__trial.rawNeeds=true;window.dispatchEvent(new Event('focus'))")
  expect(raw).to_have_text('Text service required · not configured');checks.append('Customized Raw uses backend policy, not a hardcoded free label')
  page.evaluate("window.__trial.fail=true;window.dispatchEvent(new Event('listener-type:credentials-status-changed'))")
  expect(light).to_contain_text('Text-service status unavailable');page.evaluate('window.__trial.fail=false');light.get_by_role('button',name='Retry',exact=True).click()
  expect(light).to_have_text('Text service required · not configured');checks.append('Read failure stays unknown and can retry without changing settings')
  page.evaluate("window.__trial.available=false;window.dispatchEvent(new Event('listener-type:credentials-status-changed'))")
  expect(light).to_have_text('Preview · try in the desktop app');checks.append('Preview does not claim real processing is available')
  if a.report:page.screenshot(path=str(a.report.with_suffix('.png')))
  page.get_by_role('button',name='Overview',exact=True).click();expect(page.locator('[data-style-availability]')).to_have_count(0);reads=page.evaluate('window.__trial.reads');page.evaluate("window.dispatchEvent(new Event('listener-type:credentials-status-changed'));window.dispatchEvent(new Event('focus'))");page.wait_for_timeout(100)
  assert page.evaluate('window.__trial.reads')==reads and page.evaluate('window.__trial.writes')==initial_writes
  checks.append('Unmount removes subscriptions; all status checks leave providers, active style and text services untouched')
 finally:browser.close()
report={'at':datetime.datetime.now().astimezone().isoformat(),'passed':True,'checks':checks,'scope':'Actual Style UI with controlled local readiness receipts; no native/user credential writes, cloud, audio or purchase qualification.'}
if a.report:a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(report))
