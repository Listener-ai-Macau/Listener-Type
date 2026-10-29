"""Actual React credential-read loading/success/missing/error/race checks.
Controlled status replies only, no credentials, service, device or Owner desktop.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--baseline',action='store_true');a=p.parse_args()
report={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':False,'baseline':a.baseline,'checks':[]}
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu'])
 try:
  page=browser.new_page(viewport={'width':1180,'height':900})
  page.add_init_script("""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.__credPending=[];window.__credCalls=[];
window.__credFixture=(cmd,args,mock)=>{window.__credCalls.push(cmd);if(cmd==='get_credentials')return new Promise((resolve,reject)=>window.__credPending.push({resolve,reject,done:false}));return mock();};
window.__releaseCreds=(configured,index)=>{const value={activeAsrProvider:'volcengine',activeLlmProvider:'deepseek',asrConfigured:configured,llmConfigured:configured,volcengineConfigured:configured,arkConfigured:false};window.__credPending.forEach((r,i)=>{if(!r.done&&(index===undefined||index===i)){r.done=true;r.resolve(value);}});};
""")
  def route_ipc(route):
   response=route.fetch();text=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert old in text
   route.fulfill(response=response,body=text.replace(old,'if (!isTauri) {\n    return window.__credFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),route_ipc);page.goto(a.url,wait_until='networkidle')
  services=page.locator('[data-overview-services]');page.wait_for_function('window.__credPending.length>0');expect(services).to_be_visible()
  if a.baseline:
   expect(services.get_by_text('Not configured',exact=True)).to_have_count(2)
   expect(services).to_contain_text('ARK');report['baselineBugReproduced']=True
   report['checks'].append('Before any credential reply, existing Overview incorrectly shows both missing and ARK; matches customer transient report.')
  else:
   def pending_is_honest():
    expect(services.get_by_text('Not configured',exact=True)).to_have_count(0)
    expect(services).not_to_contain_text('ARK')
    expect(services).to_contain_text(re.compile('loading',re.I))
    expect(page.get_by_role('button',name='Set up speech recognition first',exact=True)).to_have_count(0)
   pending_is_honest()
  page.evaluate('window.__releaseCreds(true)');expect(services).to_contain_text('DeepSeek');expect(services.get_by_text('Configured',exact=True)).to_have_count(2)
  report['checks'].append('Actual accepted status identifies configured DeepSeek and speech service, with no default-provider claim.')
  if not a.baseline:
   def remount():
    before=page.evaluate('window.__credPending.length');nav=page.locator('aside nav');nav.locator('button').last.click();expect(page.locator('[data-tool]')).to_have_count(5);nav.locator('button').first.click();page.wait_for_function('(n)=>window.__credPending.length>n',arg=before)
   remount();pending_is_honest();page.evaluate('window.__releaseCreds(false)');expect(services.get_by_text('Not configured',exact=True)).to_have_count(2);expect(page.get_by_role('button',name='Set up speech recognition first',exact=True)).to_have_count(1)
   report['checks'].append('Return navigation starts as loading; only a genuine missing reply exposes configuration remediation.')
   remount();pending_is_honest();page.evaluate("window.__credPending.forEach(r=>{if(!r.done){r.done=true;r.reject(new Error('controlled status read failure'));}})");expect(services).to_contain_text('Could not read credential status');expect(services.get_by_text('Not configured',exact=True)).to_have_count(0);expect(services).not_to_contain_text('ARK')
   report['checks'].append('Read failure is unavailable/read-error, never fabricated missing credentials/provider.')
   remount();pending_is_honest();before=page.evaluate('window.__credPending.length');page.evaluate("window.dispatchEvent(new Event('listener-type:credentials-status-changed'))");page.wait_for_function('(n)=>window.__credPending.length>n',arg=before);latest=page.evaluate('window.__credPending.length-1');page.evaluate('(i)=>window.__releaseCreds(true,i)',latest);expect(services.get_by_text('Configured',exact=True)).to_have_count(2);page.evaluate('window.__releaseCreds(false)');page.evaluate('new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))');expect(services.get_by_text('Configured',exact=True)).to_have_count(2)
   report['checks'].append('Newest successful status wins over an older missing reply; existing request-ownership fence is retained.')
  calls=page.evaluate('window.__credCalls');assert not any(x in calls for x in ['set_credential','try_style_pack_text','try_translation_text','prepare_local_translation','start_dictation'])
  report.update(passed=True,statusReads=calls.count('get_credentials'));page.screenshot(path=str(a.report.with_suffix('.png')))
 finally:
  browser.close();a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))
