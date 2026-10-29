"""Actual React offline route/asset controls with explicit headless IPC fixtures.
No native inference, download, cloud, hardware or owner desktop is exercised.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--screenshot',type=Path);p.add_argument('--expected-build');a=p.parse_args();checks=[]
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu'])
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
    if(cmd==='get_local_translation_status'){const status=structuredClone(window.__offlineStatus);if(args.quality==='enhanced'){status.runtimeReady=!!window.__enhancedReady;status.runtimeDownloadBytes=37178856;status.pairs=status.pairs.map(p=>({...p,downloadBytes:1133080448,ready:!!window.__enhancedReady}));}return status;}
    if(cmd==='list_saved_text_drafts')return [{id:'older-fixture',createdAt:'2020-01-01T00:00:00Z',name:'Earlier draft',original:'Earlier source',text:'An older saved draft'},...(window.__missingSaved?[]:mock())];
    if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:true};
    if(cmd==='prepare_local_translation'){
      window.__offlineStatus.preparing=true;
      return new Promise((resolve,reject)=>window.__preparePending={resolve,reject});
    }
    if(cmd==='cancel_local_translation_preparation'){
      window.__offlineStatus.preparing=false;window.__preparePending.reject('local_translation_cancelled');window.__preparePending=null;return;
    }
    if(cmd==='try_local_translation_text'){
      if(window.__offlineFail)return Promise.reject(typeof window.__offlineFail==='string'?window.__offlineFail:'local_translation_not_ready');
      return new Promise((resolve,reject)=>{const complete=()=>resolve({packId:'local_translation:'+args.sourceLanguage+':'+args.targetLanguage,packName:'fixture',outputText:args.quality==='enhanced'?'Enhanced fixture result':'Do not send the email; save the draft only.',usedTextService:false,elapsedMs:410});complete.requestId=args.requestId;complete.cancel=()=>reject('local_translation_cancelled');window.__offlinePending.push(complete);});
    }
    if(cmd==='cancel_local_translation'){const i=window.__offlinePending.findIndex(x=>x.requestId===args.requestId);if(i>=0)window.__offlinePending.splice(i,1)[0].cancel();return;}
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
   page.get_by_role('button',name='Open text workspace',exact=True).click();expect(page.locator('.ol-style-trial-input')).to_be_visible()
  def close_trial():page.locator('dialog[open]').get_by_role('button',name='Close',exact=True).first.click()
  page.get_by_role('button',name='Chinese ↔ English on this PC',exact=True).click()
  expect(page.get_by_role('combobox',name='What would you like to do?',exact=True)).to_have_value('translate')
  expect(page.get_by_role('combobox',name='Translation method',exact=True)).to_have_value('local')
  expect(page.get_by_role('combobox',name='Original language',exact=True)).to_have_value('')
  page.locator('.ol-style-trial-input').fill('保留这份输入')
  help=page.locator('[data-local-model-help]')
  expect(help.locator('summary')).to_have_text('Choosing light or enhanced')
  help.locator('summary').click()
  expect(help).to_contain_text('about 1.2 GB')
  expect(help).to_contain_text('about 4.4 GB')
  expect(help).to_contain_text('Both can misread or omit names, addresses and specialist terms')
  expect(help).to_contain_text('Selection does not download or translate')
  expect(page.locator('.ol-style-trial-input')).to_have_value('保留这份输入')
  checks.append('Light-model selection already exposes model comparison, enhanced size/CPU/RAM tradeoff and explicit preparation help without switching, downloading or translating; current source is retained')
  assert calls('prepare_local_translation')==[] and calls('try_local_translation_text')==[] and calls('try_translation_text')==[]
  close_trial();open_trial()
  expect(page.get_by_role('combobox',name='What would you like to do?',exact=True)).to_have_value('text')
  expect(page.locator('.ol-style-trial-input')).to_have_value('保留这份输入')
  close_trial()
  checks.append('Direct local entry opens the existing workspace in explicit translation mode without services/download; ordinary text entry resets task and retains input')
  open_trial();expect(page.locator('[data-style-trial-title]')).to_have_text('Text workspace');expect(page.locator('[data-translation-output-choice]')).to_have_count(0);source='不要发送邮件，只保存草稿。';page.locator('.ol-style-trial-input').fill(source)
  page.get_by_role('combobox',name='What would you like to do?',exact=True).select_option('translate')
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('English')
  engine=page.get_by_role('combobox',name='Translation method',exact=True)
  expect(engine).to_have_value('local')
  expect(page.locator('[data-translation-trial-scope]')).to_contain_text('do not change your voice translation settings')
  expect(page.get_by_role('button',name='Translate',exact=True)).to_be_disabled()
  task=page.get_by_role('combobox',name='What would you like to do?',exact=True)
  source_picker=page.get_by_role('combobox',name='Original language',exact=True)
  expect(page.locator('[data-style-trial-title]')).to_have_text('Translate to English')
  expect(source_picker).to_have_value('')
  expect(page.locator('[data-local-source-suggestion]')).to_contain_text('contains Han characters')
  for foreign in ['今日は会議です。','회의는 내일입니다.','This is a draft.']:
   page.locator('.ol-style-trial-input').fill(foreign)
   expect(page.locator('[data-local-source-suggestion]')).to_have_count(0)
   expect(source_picker).to_have_value('')
  page.locator('.ol-style-trial-input').fill(source)
  source_picker.select_option('en')
  expect(page.locator('[data-local-source-suggestion]')).to_have_count(0)
  expect(source_picker).to_have_value('en')
  source_picker.select_option('')
  page.get_by_role('button',name='Use Chinese as the source',exact=True).click()
  expect(source_picker).to_have_value('zh')
  expect(page.locator('[data-local-source-suggestion]')).to_have_count(0)
  expect(page.get_by_role('button',name='Translate',exact=True)).to_be_disabled()
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('日本語')
  task.select_option('text')
  expect(page.locator('[data-style-trial-title]')).to_have_text('Text workspace')
  expect(page.locator('[data-translation-output-choice]')).to_have_count(0)
  expect(page.locator('.ol-style-trial-input')).to_have_value(source)
  task.select_option('translate')
  expect(page.get_by_role('combobox',name='Translate output to',exact=True)).to_have_value('日本語')
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('English')
  expect(source_picker).to_have_value('zh')
  expect(page.locator('.ol-style-trial-input')).to_have_value(source)
  checks.append('Explicit task selection separates unchanged-language style from translation and preserves draft/source without a request')
  checks.append('Han-script suggestion requires explicit confirmation, excludes kana/Hangul/Latin-only text and never replaces a chosen source')
  assert calls('prepare_local_translation')==[] and calls('try_local_translation_text')==[] and calls('try_translation_text')==[]
  checks.append('Default local choice and language changes do not download, translate or call a cloud provider')
  setup=page.locator('[data-local-translation-setup]')
  setup.get_by_role('button',name=re.compile('Download offline components')).click()
  page.wait_for_function('window.__preparePending!==null')
  assert calls('prepare_local_translation')[0]['args']=={'sourceLanguage':'zh','targetLanguage':'en','quality':'standard'}
  setup.get_by_role('button',name='Cancel',exact=True).click()
  expect(setup).to_contain_text('Stopped.')
  expect(page.locator('.ol-style-trial-input')).to_have_value(source)
  assert calls('try_translation_text')==[]
  checks.append('Only explicit preparation starts exact selected pair; cancellation retains draft without cloud fallback')
  setup.get_by_role('button',name=re.compile('Download offline components')).click()
  page.wait_for_function('window.__preparePending!==null')
  page.evaluate("window.__offlineStatus.preparing=false;window.__offlineStatus.runtimeReady=true;window.__offlineStatus.pairs[0].ready=true;window.__preparePending.resolve(structuredClone(window.__offlineStatus));window.__preparePending=null")
  run=page.get_by_role('button',name='Translate',exact=True);expect(run).to_be_enabled()
  run.click();page.wait_for_function('window.__offlinePending.length===1')
  expect(task).to_be_disabled()
  expect(source_picker).to_be_disabled()
  expect(page.locator('[data-style-trial-pending]')).to_contain_text('does not request cloud translation')
  expect(page.locator('[data-style-trial-pending]')).not_to_contain_text('may still be charged')
  first=calls('try_local_translation_text')[0]['args'];assert {k:first[k] for k in ['rawText','sourceLanguage','targetLanguage','quality']}=={'rawText':source,'sourceLanguage':'zh','targetLanguage':'English','quality':'standard'}
  import uuid;uuid.UUID(first['requestId'])
  page.evaluate('window.__offlinePending.shift()()')
  output=page.locator('.ol-style-trial-result-editor');expect(output).to_have_value('Do not send the email; save the draft only.')
  options=page.locator('[data-text-options-panel]')
  expect(options).not_to_have_attribute('open','')
  expect(options.locator(':scope > summary')).to_contain_text('Chinese → English')
  expect(options.locator(':scope > summary')).to_contain_text('no translation API fee')
  for width,height in [(1180,900),(900,700)]:
   page.set_viewport_size({'width':width,'height':height})
   page.locator('[data-workspace-scroll]').evaluate('(e)=>e.scrollTop=0')
   expect(page.get_by_role('button',name='Save draft',exact=True)).to_be_in_viewport()
  page.set_viewport_size({'width':520,'height':760})
  assert page.locator('dialog[open]').evaluate('(e)=>e.scrollWidth<=e.clientWidth+1')
  page.set_viewport_size({'width':1180,'height':900})
  options.locator(':scope > summary').click()
  expect(source_picker).to_have_value('zh');expect(engine).to_have_value('local')
  expect(page.get_by_role('combobox',name='Translate output to',exact=True)).to_have_value('English')
  options.locator(':scope > summary').click()
  expect(output).to_have_value('Do not send the email; save the draft only.')
  checks.append('Successful translation folds selected settings with truthful route summary; Save stays in view at desktop/laptop sizes, narrow view has no horizontal overflow, reopening retains selections')
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
  expect(saved.get_by_label('Draft name',exact=True)).to_have_value(re.compile('Local translation'))
  expect(saved.get_by_label('Draft name',exact=True)).to_have_value(re.compile('Manually edited'))
  expect(saved.get_by_role('combobox',name='Choose a saved draft',exact=True)).not_to_have_value('older-fixture')
  saved.get_by_role('button',name='Close',exact=True).click()
  expect(output).to_have_value('Do not send the email. Keep the draft for review.')
  page.evaluate('window.__missingSaved=true')
  page.get_by_role('button',name='Open this saved draft',exact=True).click()
  saved=page.get_by_role('dialog',name='Saved drafts',exact=True)
  expect(saved.get_by_role('status')).to_contain_text('could not be found')
  expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_count(0)
  expect(saved.get_by_role('combobox',name='Choose a saved draft',exact=True)).to_have_value('')
  saved.get_by_role('combobox',name='Choose a saved draft',exact=True).select_option('older-fixture')
  expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_value('An older saved draft')
  saved.get_by_role('button',name='Close',exact=True).click();page.evaluate('window.__missingSaved=false')
  expect(page.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(0)
  expect(page.locator('[data-style-trial-retention]')).to_contain_text('Not saved')
  page.get_by_role('button',name='Save draft',exact=True).click()
  page.get_by_role('button',name='Open this saved draft',exact=True).click()
  saved=page.get_by_role('dialog',name='Saved drafts',exact=True)
  expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_value('Do not send the email. Keep the draft for review.')
  saved.get_by_role('button',name='Delete draft',exact=True).click()
  saved.get_by_role('alert').get_by_role('button',name='Delete draft',exact=True).click()
  saved.get_by_role('button',name='Close',exact=True).click()
  expect(page.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(0)
  expect(page.get_by_role('button',name='Save draft',exact=True)).to_be_enabled()
  expect(output).to_have_value('Do not send the email. Keep the draft for review.')
  expect(page.locator('[data-style-trial-retention]')).to_contain_text('Not saved')
  checks.append('Explicit save opens the returned draft directly even when the list starts with an older record; closing keeps the edited workspace')
  checks.append('Missing requested draft never silently selects another; explicit deletion clears workspace saved state while retaining text')
  options.locator(':scope > summary').click()
  page.get_by_role('combobox',name='Original language',exact=True).select_option('en')
  expect(run).to_be_disabled();expect(output).to_have_count(0)
  page.get_by_role('combobox',name='Original language',exact=True).select_option('zh')
  page.locator('.ol-style-trial-input').fill('另一份原文，不要丢失。');page.evaluate('window.__offlineFail=true');run.click()
  expect(page.locator('dialog[open] [role=alert]')).to_contain_text('missing or damaged')
  expect(options).to_have_attribute('open','')
  expect(page.locator('.ol-style-trial-input')).to_have_value('另一份原文，不要丢失。')
  expect(page.locator('.ol-style-trial-result-editor')).to_have_count(0);assert calls('try_translation_text')==[]
  checks.append('Source change immediately fences readiness/results; local failure keeps source with no paid retry')
  for failure in ['local_translation_time_fidelity','local_translation_date_fidelity']:
   page.evaluate('(failure)=>window.__offlineFail=failure',failure);run.click()
   expect(page.locator('dialog[open] [role=alert]')).to_contain_text('date, time or paragraph structure')
   expect(page.locator('.ol-style-trial-input')).to_have_value('另一份原文，不要丢失。')
   expect(page.locator('.ol-style-trial-result-editor')).to_have_count(0)
   expect(engine).to_have_value('local');assert calls('try_translation_text')==[]
  checks.append('Time/year fidelity rejection retains the exact source, accepts no partial result and never silently uploads or selects cloud; the existing explicit method selector remains available')
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
  page.get_by_role('combobox',name='What would you like to do?',exact=True).select_option('translate')
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('English')
  page.get_by_role('combobox',name='Translation method',exact=True).select_option('cloud')
  page.locator('.ol-style-trial-input').fill('一份明确选择云端处理的草稿。')
  page.get_by_role('button',name='Translate',exact=True).click();page.wait_for_function('window.__cloudPending.length===1');page.evaluate('window.__cloudPending.shift()()')
  expect(page.locator('.ol-style-trial-result-editor')).to_have_value('Cloud fixture result')
  summary=page.locator('[data-text-options-panel] summary');expect(summary).to_contain_text('English');expect(summary).to_contain_text('existing cloud service');expect(summary).not_to_contain_text('Choose');expect(page.get_by_role('combobox',name='Original language',exact=True)).to_have_count(0)
  page.get_by_role('button',name='Save draft',exact=True).click();page.get_by_role('button',name='Open this saved draft',exact=True).click();saved=page.get_by_role('dialog',name='Saved drafts',exact=True)
  expect(saved.get_by_label('Draft name',exact=True)).to_have_value(re.compile('Cloud translation'));expect(saved.get_by_label('Draft name',exact=True)).not_to_have_value(re.compile('Local translation|Manually edited'))
  expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_value('Cloud fixture result')
  checks.append('New draft default names use the actual local/cloud result and explicit manual edit; cloud summary names target/service without a nonexistent source selector')


  page.get_by_role('dialog',name='Saved drafts',exact=True).get_by_role('button',name='Close',exact=True).click()
  close_trial();open_trial();page.evaluate('window.__offlineFail=false')
  page.get_by_role('combobox',name='What would you like to do?',exact=True).select_option('translate')
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('English')
  page.get_by_role('combobox',name='Original language',exact=True).select_option('zh')
  page.locator('.ol-style-trial-input').fill('保留此原文，先比较模型再取消。')
  run=page.get_by_role('button',name='Translate',exact=True);run.click();page.wait_for_function('window.__offlinePending.length===1');page.evaluate('window.__offlinePending.shift()()')
  expect(page.locator('.ol-style-trial-result-editor')).to_have_value('Do not send the email; save the draft only.')
  options=page.locator('[data-text-options-panel]');options.locator(':scope > summary').click()
  quality=page.get_by_role('combobox',name='Local model',exact=True);count=len(calls('prepare_local_translation'));translated=len(calls('try_local_translation_text'))
  quality.select_option('enhanced');expect(run).to_be_disabled();expect(page.locator('.ol-style-trial-result-editor')).to_have_count(0)
  expect(page.locator('[data-local-translation-setup]')).to_contain_text('available RAM')
  assert len(calls('prepare_local_translation'))==count and len(calls('try_local_translation_text'))==translated
  quality.select_option('standard');expect(run).to_be_enabled();expect(page.locator('.ol-style-trial-result-editor')).to_have_value('Do not send the email; save the draft only.')
  quality.select_option('enhanced');page.get_by_role('button',name=re.compile('Download offline components')).click()
  page.wait_for_function('window.__preparePending!==null');assert calls('prepare_local_translation')[-1]['args']['quality']=='enhanced'
  page.evaluate("window.__enhancedReady=true;window.__offlineStatus.preparing=false;const s=structuredClone(window.__offlineStatus);s.runtimeReady=true;s.pairs=s.pairs.map(p=>({...p,ready:true}));window.__preparePending.resolve(s);window.__preparePending=null")
  expect(run).to_be_enabled();run.click();page.wait_for_function('window.__offlinePending.length===1')
  enhanced=calls('try_local_translation_text')[-1]['args'];assert enhanced['quality']=='enhanced'
  page.locator('[data-style-trial-pending]').get_by_role('button',name='Cancel',exact=True).click()
  expect(page.locator('[data-style-trial-pending]')).to_have_count(0)
  assert calls('cancel_local_translation')[-1]['args']['requestId']==enhanced['requestId']
  expect(page.locator('.ol-style-trial-input')).to_have_value('保留此原文，先比较模型再取消。')
  expect(page.locator('.ol-style-trial-result-editor')).to_have_count(0)
  run.click();page.wait_for_function('window.__offlinePending.length===1');closing=calls('try_local_translation_text')[-1]['args']['requestId'];close_trial()
  page.wait_for_function('(id)=>window.__offlineCalls.some(x=>x.cmd==="cancel_local_translation"&&x.args.requestId===id)',arg=closing)
  page.wait_for_function('window.__offlinePending.length===0');open_trial();expect(page.locator('.ol-style-trial-input')).to_have_value('保留此原文，先比较模型再取消。')
  assert len(calls('try_translation_text'))==2
  checks.append('Model readiness and cached results are isolated by quality; selecting enhanced neither downloads nor translates; explicit preparation/request carry quality; Cancel and Close address the exact request and retain source without cloud fallback')

 except Exception as error:
  if a.screenshot:
   a.screenshot.parent.mkdir(exist_ok=True,parents=True);page.screenshot(path=str(a.screenshot))
  a.report.parent.mkdir(exist_ok=True,parents=True)
  a.report.write_text(json.dumps({'at':datetime.datetime.now().astimezone().isoformat(),'passed':False,'error':str(error),'checksCompleted':checks,'viewport':page.viewport_size,'layout':page.evaluate("Array.from(document.querySelectorAll('dialog[open],.ol-style-trial-options-panel,.ol-style-trial-result-editor,.ol-style-trial-primary-actions')).map(e=>({tag:e.tagName,classes:e.className,rect:e.getBoundingClientRect().toJSON()}))")},ensure_ascii=False,indent=2)+'\n',encoding='utf8')
  raise
 finally:browser.close()
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':True,'checks':checks}
a.report.parent.mkdir(exist_ok=True,parents=True);a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf8');print(json.dumps(r,ensure_ascii=False))
