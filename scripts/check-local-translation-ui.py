"""Actual React offline route/asset controls with explicit headless IPC fixtures.
No native inference, download, cloud, hardware or owner desktop is exercised.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--screenshot',type=Path);p.add_argument('--expected-build');a=p.parse_args();checks=[]
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=browser.new_page(viewport={'width':1180,'height':900})
  page.add_init_script("""
   localStorage.setItem('ol.locale','en');
   sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
   sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
   window.__offlineCalls=[];window.__offlinePending=[];window.__cloudPending=[];window.__preparePending=null;window.__offlineFail=false;
   window.__offlineStatus={supported:true,runtimeReady:false,runtimeDownloadBytes:38481734,pairs:[{source:'zh',target:'en',downloadBytes:74481402,ready:false},{source:'en',target:'zh',downloadBytes:70743021,ready:false}],preparing:false,receivedBytes:0,expectedBytes:112963136};
   window.__offlineFixture=(cmd,args,mock)=>{
    window.__offlineCalls.push({cmd,args});
    if(cmd==='get_local_translation_status')return structuredClone(window.__offlineStatus);
    if(cmd==='list_saved_text_drafts')return [{id:'older-fixture',createdAt:'2020-01-01T00:00:00Z',name:'Earlier draft',original:'Earlier source',text:'An older saved draft'},...mock()];
    if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:true};
    if(cmd==='prepare_local_translation'){
      window.__offlineStatus.preparing=true;
      return new Promise((resolve,reject)=>window.__preparePending={resolve,reject});
    }
    if(cmd==='cancel_local_translation_preparation'){
      window.__offlineStatus.preparing=false;window.__preparePending.reject('local_translation_cancelled');window.__preparePending=null;return;
    }
    if(cmd==='try_local_translation_text'){
      if(window.__offlineFail)return Promise.reject('local_translation_not_ready');
      return new Promise(resolve=>window.__offlinePending.push(()=>resolve({packId:'local_translation:'+args.sourceLanguage+':'+args.targetLanguage,packName:'fixture',outputText:'Do not send the email; save the draft only.',usedTextService:false,elapsedMs:410})));
    }
    if(cmd==='try_translation_text')return new Promise(resolve=>window.__cloudPending.push(()=>resolve({packId:'translation:'+args.targetLanguage,packName:args.targetLanguage,outputText:'Cloud fixture result',usedTextService:true,elapsedMs:750})));
    return mock();
   };
  """)
  def route_ipc(route):
   response=route.fetch();text=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert old in text
   route.fulfill(response=response,body=text.replace(old,'if (!isTauri) {\n    return window.__offlineFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),route_ipc)
  page.goto(a.url,wait_until='networkidle')
  if a.expected_build:
   expect(page.locator('[data-app-build]')).to_have_attribute('data-app-build',a.expected_build)
   expect(page.locator('[data-app-build]')).to_contain_text('build'+a.expected_build)
  # Locale/first-use settings synchronization belongs to app startup, not this
  # workspace. Keep its calls, then measure actual user actions from this point.
  baseline=page.evaluate('window.__offlineCalls.length')
  def calls(cmd):return page.evaluate('(name)=>window.__offlineCalls.filter(x=>x.cmd===name)',cmd)
  def open_trial():
   page.get_by_role('button',name='Try typed text',exact=True).click();expect(page.locator('#style-trial-input')).to_be_visible()
  def close_trial():page.locator('dialog[open]').get_by_role('button',name='Close',exact=True).first.click()
  open_trial();source='不要发送邮件，只保存草稿。';page.locator('#style-trial-input').fill(source)
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('English')
  engine=page.get_by_role('combobox',name='Translation method',exact=True)
  expect(engine).to_have_value('local')
  expect(page.locator('[data-translation-trial-scope]')).to_contain_text('do not change your voice translation settings')
  expect(page.get_by_role('button',name='Translate',exact=True)).to_be_disabled()
  page.get_by_role('combobox',name='Original language',exact=True).select_option('zh')
  expect(page.get_by_role('button',name='Translate',exact=True)).to_be_disabled()
  assert calls('prepare_local_translation')==[] and calls('try_local_translation_text')==[] and calls('try_translation_text')==[]
  checks.append('Default local choice and language changes do not download, translate or call a cloud provider')
  setup=page.locator('[data-local-translation-setup]')
  setup.get_by_role('button',name=re.compile('Download offline components')).click()
  page.wait_for_function('window.__preparePending!==null')
  assert calls('prepare_local_translation')[0]['args']=={'sourceLanguage':'zh','targetLanguage':'en'}
  setup.get_by_role('button',name='Cancel',exact=True).click()
  expect(setup).to_contain_text('Download stopped')
  expect(page.locator('#style-trial-input')).to_have_value(source)
  assert calls('try_translation_text')==[]
  checks.append('Only explicit preparation starts exact selected pair; cancellation retains draft without cloud fallback')
  setup.get_by_role('button',name=re.compile('Download offline components')).click()
  page.wait_for_function('window.__preparePending!==null')
  page.evaluate("window.__offlineStatus.preparing=false;window.__offlineStatus.runtimeReady=true;window.__offlineStatus.pairs[0].ready=true;window.__preparePending.resolve(structuredClone(window.__offlineStatus));window.__preparePending=null")
  run=page.get_by_role('button',name='Translate',exact=True);expect(run).to_be_enabled()
  run.click();page.wait_for_function('window.__offlinePending.length===1')
  expect(page.locator('[data-style-trial-pending]')).to_contain_text('does not request cloud translation')
  expect(page.locator('[data-style-trial-pending]')).not_to_contain_text('may still be charged')
  assert calls('try_local_translation_text')[0]['args']=={'rawText':source,'sourceLanguage':'zh','targetLanguage':'English'}
  page.evaluate('window.__offlinePending.shift()()')
  output=page.locator('.ol-style-trial-result-editor');expect(output).to_have_value('Do not send the email; save the draft only.')
  expect(page.locator('.ol-style-trial-result-meta')).to_contain_text('Local translation')
  expect(page.locator('[data-style-trial-service-notice]')).to_contain_text('translated on this computer')
  output.fill('Do not send the email. Keep the draft for review.')
  expect(output).to_have_value('Do not send the email. Keep the draft for review.')
  if a.screenshot:
   a.screenshot.parent.mkdir(exist_ok=True,parents=True);page.screenshot(path=str(a.screenshot))
  assert calls('try_translation_text')==[]
  checks.append('Explicit local request shows editable actual-route receipt and original/result comparison without cloud request')
  writes=page.evaluate("(start)=>window.__offlineCalls.slice(start).filter(x=>/^(save_|set_|update_settings|copy_style_trial|insert_style_trial)/.test(x.cmd)).map(x=>({cmd:x.cmd,translationEngine:x.args?.prefs?.translationEngine,translationSource:x.args?.prefs?.translationSourceLanguage,translationTarget:x.args?.prefs?.translationTargetLanguage}))",baseline)
  assert not writes,writes
  checks.append('No automatic saving, copying, insertion, style activation, preferences or device writes')
  page.get_by_role('button',name='Save draft',exact=True).click()
  page.get_by_role('button',name='Open this saved draft',exact=True).click()
  saved=page.get_by_role('dialog',name='Saved drafts',exact=True)
  expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_value('Do not send the email. Keep the draft for review.')
  expect(saved.get_by_role('combobox',name='Choose a saved draft',exact=True)).not_to_have_value('older-fixture')
  saved.get_by_role('button',name='Close',exact=True).click()
  expect(output).to_have_value('Do not send the email. Keep the draft for review.')
  checks.append('Explicit save opens the returned draft directly even when the list starts with an older record; closing keeps the edited workspace')
  page.get_by_role('combobox',name='Original language',exact=True).select_option('en')
  expect(run).to_be_disabled();expect(output).to_have_count(0)
  page.get_by_role('combobox',name='Original language',exact=True).select_option('zh')
  page.locator('#style-trial-input').fill('另一份原文，不要丢失。');page.evaluate('window.__offlineFail=true');run.click()
  expect(page.locator('dialog[open] [role=alert]')).to_contain_text('missing or damaged')
  expect(page.locator('#style-trial-input')).to_have_value('另一份原文，不要丢失。')
  expect(page.locator('.ol-style-trial-result-editor')).to_have_count(0);assert calls('try_translation_text')==[]
  checks.append('Source change immediately fences readiness/results; local failure keeps source with no paid retry')
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('日本語')
  expect(run).to_be_disabled();expect(setup).to_contain_text('Chinese ↔ English')
  expect(engine).to_have_value('local');assert calls('try_translation_text')==[]
  checks.append('Unavailable language stays visibly unsupported and never silently changes engine')
  engine.select_option('cloud');expect(run).to_be_enabled();run.click();page.wait_for_function('window.__cloudPending.length===1')
  expect(page.locator('[data-style-trial-pending]')).to_contain_text('may still be charged')
  assert len(calls('try_translation_text'))==1
  close_trial();open_trial();page.evaluate('window.__cloudPending.shift()()')
  expect(page.locator('.ol-style-trial-result-editor')).to_have_count(0)
  checks.append('Cloud is called only after explicit engine selection; closed workspace cannot receive its late result')
 finally:browser.close()
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':True,'checks':checks}
a.report.parent.mkdir(exist_ok=True,parents=True);a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf8');print(json.dumps(r,ensure_ascii=False))
