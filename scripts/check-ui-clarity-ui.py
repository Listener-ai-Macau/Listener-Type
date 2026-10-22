"""Controlled frontend layout/navigation checks for observed customer feedback.

Headless browser fixtures only: no user desktop, recordings, cloud or clipboard.
Appearance preference remains a separate customer judgment.
"""
import argparse, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',required=True,type=Path);a=p.parse_args()
report={'scope':__doc__,'passed':False,'checks':[]}
fixture_js="""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.__clarityCalls=[];window.__historyMode='empty';window.__readinessMode='ready';
window.__clarityFixture=(cmd,args,mock)=>{
 window.__clarityCalls.push({cmd,args});
 if(cmd==='list_history'){
  if(window.__historyMode==='error')throw Error('read unavailable');
  return window.__historyMode==='empty'?[]:[{id:'retained',createdAt:new Date().toISOString(),rawTranscript:'Original recording',finalText:'Retained recording',mode:'raw',durationMs:1000,insertStatus:'pasteSent'}];
 }
 if(cmd==='get_style_text_trial_readiness'){
  const value={available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:window.__readinessMode==='ready'};
  if(window.__readinessMode==='checking')return new Promise(resolve=>window.__finishReadiness=()=>resolve({...value,textServiceConfigured:true}));
  if(window.__readinessMode==='error')throw Error('readiness unavailable');
  return value;
 }
 if(cmd==='try_style_pack_text')return {packId:args.stylePackId,packName:args.stylePackId,outputText:args.rawText,usedTextService:false,elapsedMs:1};
 return mock();
};
"""

def settled(page):
 page.wait_for_function("""()=>{const e=document.querySelector('main > .ol-thinscroll');return e&&getComputedStyle(e).opacity==='1'&&!e.getAnimations().some(a=>a.playState==='running');}""")

with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  context=browser.new_context(viewport={'width':1240,'height':800},device_scale_factor=2)
  context.route(re.compile(r'^https?://'),lambda r:r.continue_() if re.match(r'^https?://(localhost|127\.0\.0\.1)(:|/)',r.request.url) else r.abort())
  page=context.new_page();page.set_default_timeout(6000);page.add_init_script(fixture_js)
  def fixture(route):
   response=route.fetch();source=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert source.count(old)==1
   route.fulfill(response=response,body=source.replace(old,'if (!isTauri) {\n    return window.__clarityFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  page.goto(a.url,wait_until='networkidle');settled(page)
  utilities=page.locator('.ol-overview-utilities');expect(utilities).not_to_have_attribute('open','')
  expect(utilities.get_by_role('button',name='Saved drafts',exact=True)).not_to_be_visible()
  expect(page.get_by_role('button',name='Try typed text',exact=True)).to_be_visible()
  page.screenshot(path=str(a.report.with_name(a.report.stem+'-overview.png')))
  utilities.locator('summary').click()
  for name in ['Saved drafts','Transcribe recorded audio','Try the four keys','Design your keys','Use my saved phrases']:
   utilities.get_by_role('button',name=name,exact=True).click()
   close=page.get_by_role('button',name='Close',exact=True).last;expect(close).to_be_visible();close.click()
  utilities.locator('summary').click();report['checks'].append('secondary Overview utilities are collapsed initially and all five existing dialogs remain reachable')
  page.get_by_role('button',name='Try typed text',exact=True).click();trial=page.locator('dialog.ol-style-trial')
  expect(trial.get_by_label('Your text',exact=True)).to_be_enabled()
  expect(trial.locator('.ol-style-trial-availability')).to_have_count(0)
  gap=trial.evaluate("e=>e.querySelector('label[for=style-trial-input]').getBoundingClientRect().top-e.querySelector('[data-style-trial-purpose]').getBoundingClientRect().bottom")
  assert 0<=gap<45,gap
  expect(trial.locator('[data-style-trial-service-notice]')).to_contain_text('may incur charges')
  help=trial.locator('details').filter(has_text='How this trial uses your text')
  expect(help).not_to_have_attribute('open','')
  expect(trial.get_by_text('Make shorter uses your text service.',exact=False)).not_to_be_visible()
  text='Reviewed text — 条件😀\nKeep Thursday at three.'
  trial.get_by_label('Your text',exact=True).fill(text);trial.get_by_role('button',name='Try this text',exact=True).click()
  expect(trial.get_by_label('Result text',exact=True)).to_have_value(text)
  boxes=trial.evaluate("e=>Object.fromEntries(['#style-trial-input','.ol-style-trial-result-editor'].map(s=>[s,Math.round(e.querySelector(s).getBoundingClientRect().height)]))")
  assert abs(boxes['#style-trial-input']-boxes['.ol-style-trial-result-editor'])<=2,boxes
  for label in ['Copy result','Save draft']:
   box=trial.get_by_role('button',name=label,exact=True).bounding_box();assert box and box['y']>=0 and box['y']+box['height']<=800,box
  trial.screenshot(path=str(a.report.with_name(a.report.stem+'-text-result.png')))
  tops=trial.evaluate("e=>[e.querySelector('#style-trial-input'),e.querySelector('.ol-style-trial-result-editor')].map(x=>x.getBoundingClientRect().top)")
  assert abs(tops[0]-tops[1])<3,tops
  help.locator('summary').click();expect(help).to_contain_text('Make shorter uses your text service');help.locator('summary').click()
  report['checks'].append({'editorTopAlignment':tops,'longInstructionsCollapsedButReachable':True})
  report['checks'].append({'readyWorkspaceReservedGap':gap,'balancedEditorHeights':boxes,'copyAndSaveInViewport':True})
  trial.get_by_role('button',name='Save draft',exact=True).click();expect(trial.get_by_role('button',name='Saved draft',exact=True)).to_be_disabled()
  trial.get_by_role('button',name='Close',exact=True).click();page.locator('aside nav').get_by_role('button',name='History',exact=True).click();expect(page.locator('[data-history-empty]')).to_be_visible();settled(page)
  expect(page.get_by_text('Last 7 days',exact=True)).to_have_count(0);expect(page.get_by_role('button',name='Clear',exact=True)).to_have_count(0)
  page.screenshot(path=str(a.report.with_name(a.report.stem+'-empty-history.png')))
  page.get_by_role('button',name='Saved drafts',exact=True).first.click();saved=page.get_by_role('dialog',name='Saved drafts',exact=True)
  label=saved.get_by_label('Choose a saved draft',exact=True).locator('option').first.inner_text();assert label.startswith('Reviewed text — 条件😀'),label
  expect(saved.locator('section > p.ol-device-layout-hint')).to_have_count(0)
  expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_value(text);saved.screenshot(path=str(a.report.with_name(a.report.stem+'-saved-drafts.png')));saved.get_by_role('button',name='Close',exact=True).click()
  page.evaluate("window.__historyMode='populated'");page.get_by_role('button',name='Refresh',exact=True).first.click();expect(page.locator('[data-history-empty]')).to_have_count(0);expect(page.get_by_text('Retained recording',exact=True).last).to_be_visible()
  page.evaluate("window.__historyMode='error'");page.get_by_role('button',name='Refresh',exact=True).first.click();expect(page.get_by_role('button',name='Retry',exact=True)).to_be_visible();expect(page.locator('[data-history-empty]')).to_have_count(0)
  report['checks'].append('empty history uses one state and opens the exact saved draft; populated and read-error history retain their normal views')
  page.locator('aside nav').get_by_role('button',name='Overview',exact=True).click();settled(page)
  for mode in ['checking','missing','error']:
   page.evaluate('(mode)=>window.__readinessMode=mode',mode)
   page.get_by_role('button',name='Try typed text',exact=True).click();trial=page.locator('dialog.ol-style-trial');expect(trial.get_by_label('Your text',exact=True)).to_be_enabled()
   if mode=='checking':
    expect(trial.get_by_role('status')).to_contain_text('Checking');page.evaluate('window.__finishReadiness()');expect(trial.locator('.ol-style-trial-availability')).to_have_count(0)
   elif mode=='missing':
    trial.get_by_label('Trial style',exact=True).select_option('builtin.light');expect(trial.get_by_role('status')).to_contain_text('text service')
   else:expect(trial.get_by_role('alert')).to_be_visible()
   trial.get_by_role('button',name='Close',exact=True).click()
  report['checks'].append('checking, missing-service and readiness errors remain visible; drafting stays enabled')
  assert not [x for x in page.evaluate('window.__clarityCalls') if re.match(r'^(update_preferences|set_selected_device|start_dictation|clear_history|delete_history_entry|insert_)',x['cmd'])]
  report['passed']=True;context.close()
 except Exception as e:
  report['error']=repr(e);raise
 finally:
  browser.close();a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))
