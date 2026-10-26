"""Customer text-to-translation route, explicit request, editable output and failure.
Headless actual React with isolated IPC fixtures; not native cloud translation.
"""
import argparse,json,re,datetime
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args();checks=[]
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=browser.new_page(viewport={'width':1180,'height':1000})
  page.add_init_script("""
   localStorage.setItem('ol.locale','en');
   sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
   sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
   window.__translationCalls=[];window.__pendingTranslation=[];window.__authFailure=false;
   window.__translationFixture=(cmd,args,mock)=>{
    window.__translationCalls.push({cmd,args});
    if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:true};
    if(cmd==='try_style_pack_text')return {packId:args.stylePackId,packName:args.stylePackId,outputText:args.rawText,usedTextService:false,elapsedMs:1};
    if(cmd==='try_translation_text'){
     if(window.__authFailure)return Promise.reject('invalid response: status 401 AuthenticationError');
     return new Promise(resolve=>window.__pendingTranslation.push(()=>resolve({packId:'translation:'+args.targetLanguage,packName:args.targetLanguage,outputText:'Meeting tomorrow at 3 pm; please bring the product samples.',usedTextService:true,elapsedMs:750})));
    }
    return mock();
   };
  """)
  def fixture(route):
   response=route.fetch();text=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert old in text
   route.fulfill(response=response,body=text.replace(old,'if (!isTauri) {\n    return window.__translationFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture);page.goto(a.url,wait_until='networkidle')
  def open_trial():
   page.get_by_role('button',name='Try typed text',exact=True).click();expect(page.locator('.ol-style-trial-input')).to_be_visible()
  def close_trial():page.locator('dialog[open]').get_by_role('button',name='Close',exact=True).first.click()
  def request_count():return page.evaluate("window.__translationCalls.filter(x=>x.cmd==='try_translation_text').length")
  open_trial();source='明天下午三点开会，请带上产品样品。';page.locator('.ol-style-trial-input').fill(source)
  page.get_by_role('combobox',name='What would you like to do?',exact=True).select_option('translate')
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('English');page.get_by_role('combobox',name='Translation method',exact=True).select_option('cloud')
  expect(page.locator('.ol-style-trial-input')).to_have_value(source);assert request_count()==0
  checks.append('Selecting English retains draft and does not call a service')
  expect(page.locator('[data-style-trial-service-notice]')).to_be_visible()
  page.get_by_role('button',name='Translate',exact=True).click();page.wait_for_function('window.__pendingTranslation.length===1')
  assert page.evaluate("window.__translationCalls.filter(x=>x.cmd==='try_translation_text')[0].args")=={'rawText':source,'targetLanguage':'English'}
  page.evaluate('window.__pendingTranslation.shift()()');out=page.locator('.ol-style-trial-result-editor')
  expect(out).to_have_value('Meeting tomorrow at 3 pm; please bring the product samples.')
  out.fill('Please bring the samples to the meeting tomorrow at 3 pm.');expect(out).to_have_value('Please bring the samples to the meeting tomorrow at 3 pm.')
  checks.append('Explicit translation route shows editable source/output comparison')
  assert not page.evaluate("window.__translationCalls.some(x=>['insert_style_trial_result','copy_style_trial_result','save_text_draft','set_active_style_pack','set_selected_device_key_profile'].includes(x.cmd))")
  checks.append('No automatic insertion, copying, saving, style activation or device write')
  page.locator('[data-text-options-panel] > summary').click()
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('日本語');page.get_by_role('button',name='Translate',exact=True).click();page.wait_for_function('window.__pendingTranslation.length===1');close_trial();open_trial()
  page.evaluate('window.__pendingTranslation.shift()()');expect(page.locator('.ol-style-trial-result-editor')).to_have_count(0)
  checks.append('Closed workspace cannot accept a delayed translation result into its replacement')
  page.get_by_role('combobox',name='What would you like to do?',exact=True).select_option('translate')
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('English');page.get_by_role('combobox',name='Translation method',exact=True).select_option('cloud');page.evaluate('window.__authFailure=true');page.get_by_role('button',name='Translate',exact=True).click()
  expect(page.locator('dialog[open] [role=alert]')).to_contain_text('rejected its credentials');expect(page.locator('.ol-style-trial-input')).to_have_value(source);expect(page.locator('.ol-style-trial-result-editor')).to_have_count(0)
  checks.append('401 is visible and keeps the source; no fake successful raw translation')
  close_trial();page.evaluate('window.__authFailure=false');open_trial()
  original='Original source — 条件😀\nKeep the condition.';reviewed='Reviewed source — 条件😀\nKeep the condition.'
  page.locator('.ol-style-trial-input').fill(original);page.get_by_role('button',name='Try this text',exact=True).click()
  page.get_by_label('Result text',exact=True).fill(reviewed)
  before=page.evaluate('window.__translationCalls.length');page.get_by_role('button',name='Translate this result…',exact=True).click()
  expect(page.locator('.ol-style-trial-input')).to_have_value(reviewed)
  previous=page.locator('[data-text-previous-step]');expect(previous.locator('pre').first).to_have_text(reviewed)
  previous.locator('summary').click();expect(previous.locator('pre').nth(1)).to_have_text(original)
  new_calls=page.evaluate('window.__translationCalls')[before:]
  assert not any(x['cmd'].startswith('try_') or x['cmd'] in {'save_text_draft','copy_style_trial_result','insert_style_trial_result'} for x in new_calls)
  expect(page.get_by_label('Result text',exact=True)).to_have_count(0)
  checks.append('explicit continuation carries the exact edited Unicode/newline result and labelled previous source, without automatic processing/save/copy or a stale current result')
  page.get_by_role('combobox',name='Translation method',exact=True).select_option('cloud');page.get_by_role('button',name='Translate',exact=True).click()
  page.wait_for_function('window.__pendingTranslation.length===1');page.evaluate('window.__pendingTranslation.shift()()')
  expect(page.locator('[data-translation-refine-hint]')).to_contain_text('does not compare it with the original')
  out=page.get_by_label('Result text',exact=True);out.fill('x'*4001)
  next_step=page.get_by_role('button',name='Refine this translation…',exact=True);expect(next_step).to_be_disabled();expect(out).to_have_value('x'*4001)
  edited='Reviewed English — 条件😀\nDo not add a promise.';out.fill(edited);before=page.evaluate('window.__translationCalls.length');next_step.click()
  expect(page.get_by_role('combobox',name='What would you like to do?',exact=True)).to_have_value('text');expect(page.locator('.ol-style-trial-input')).to_have_value(edited)
  expect(previous.locator('pre').first).to_have_text(edited);expect(previous.locator('details')).to_have_attribute('open','');expect(previous.locator('pre').nth(1)).to_be_visible();expect(previous.locator('pre').nth(1)).to_have_text(reviewed)
  expect(page.locator('[data-translation-refine-hint]')).to_contain_text('does not compare it with the original')
  assert not any(x['cmd'].startswith('try_') or x['cmd']=='save_text_draft' for x in page.evaluate('window.__translationCalls')[before:])
  page.get_by_role('button',name='Try this text',exact=True).click();expect(page.get_by_label('Result text',exact=True)).to_have_value(edited)
  page.get_by_role('button',name='Save draft',exact=True).click();expect(page.get_by_role('button',name='Saved draft',exact=True)).to_be_disabled()
  saved=[x['args'] for x in page.evaluate('window.__translationCalls') if x['cmd']=='save_text_draft'];assert saved[-1]['original']==edited and saved[-1]['text']==edited
  checks.append('translation-to-refinement is explicit, rejects over-limit handoff without truncation, retains the correct previous source/output and saves only the separately submitted current input/result')
  close_trial();open_trial();expect(page.locator('[data-text-previous-step]')).to_have_count(0)
  checks.append('closing clears session-only handoff references; no new persistence or recording source is introduced')
 finally:browser.close()
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':True,'checks':checks};a.report.parent.mkdir(exist_ok=True,parents=True);a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf8');print(json.dumps(r,ensure_ascii=False))
