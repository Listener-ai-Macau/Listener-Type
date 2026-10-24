"""Actual React local-translation result editing with explicit IPC fixtures.
This exercises wording controls and paid-service isolation, not model quality.
"""
import argparse, datetime, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True)
p.add_argument('--expected-build');p.add_argument('--capture-before',action='store_true')
p.add_argument('--check-undo',action='store_true')
p.add_argument('--check-context',action='store_true')
p.add_argument('--check-boundaries',action='store_true')
a=p.parse_args();checks=[]
source='抱歉，我今天下午临时有事，能把会议改到明天上午十点吗？如果不方便，周五也可以。'
machine="I'm sorry. I have a temporary appointment this afternoon. Can I move the meeting to 10:00 tomorrow morning? If it's not convenient, Friday's fine."
revised=machine.replace('Can I move','Could I please move')
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=b.new_page(viewport={'width':1280,'height':960})
  page.add_init_script("""localStorage.setItem('ol.locale','en');window.__wordingCalls=[];
  window.__wordingFixture=(cmd,args,mock)=>{
   window.__wordingCalls.push({cmd,args});
   if(cmd==='get_credentials')return {activeAsrProvider:'volcengine',activeLlmProvider:'ark',asrConfigured:false,llmConfigured:true,volcengineConfigured:false,arkConfigured:true};
   if(cmd==='get_local_translation_status')return {supported:true,runtimeReady:true,runtimeDownloadBytes:38481734,pairs:[{source:'zh',target:'en',downloadBytes:74481402,ready:true},{source:'en',target:'zh',downloadBytes:70743021,ready:true}],preparing:false,receivedBytes:0,expectedBytes:0};
   if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:false,textServiceConfigured:true};
   if(cmd==='try_local_translation_text')return {packId:'local_translation:zh:en',packName:'Local translation',outputText:window.__wordingOutput??MACHINE,usedTextService:false,elapsedMs:2300};
   if(cmd==='suggest_polite_text_edits'||cmd==='try_translation_text')throw new Error('Offline task must not call a text service');
   return mock();
  };""".replace('MACHINE',json.dumps(machine)))
  def ipc(route):
   response=route.fetch();s=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert old in s
   route.fulfill(response=response,body=s.replace(old,'if (!isTauri) {\n    return window.__wordingFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),ipc)
  page.goto(a.url,wait_until='networkidle')
  if a.expected_build:expect(page.locator('[data-app-build]')).to_have_attribute('data-app-build',a.expected_build)
  expect(page.locator('.ol-overview-text-entry')).to_contain_text('one-time component download, no API key or translation fee')
  checks.append({'name':'Overview explains local translation availability, preparation and fee boundary before entry','passed':True})
  page.get_by_role('button',name='Try typed text',exact=True).click()
  page.locator('#style-trial-input').fill(source)
  page.get_by_role('combobox',name='Translate output to',exact=True).select_option('English')
  page.get_by_role('combobox',name='Original language',exact=True).select_option('zh')
  page.get_by_role('button',name='Translate',exact=True).click()
  output=page.locator('.ol-style-trial-result-editor');expect(output).to_have_value(machine)
  wording=page.get_by_role('button',name='Wording options',exact=True)
  available=wording.count()==1
  checks.append({'name':'Offline English output exposes explicit local wording options','passed':available})
  if available:
   wording.click();panel=page.get_by_role('region',name='Request wording',exact=True)
   expect(panel).to_contain_text('Could I please move the meeting to 10:00 tomorrow morning?')
   expect(panel.get_by_role('button',name='Ask text service for alternatives',exact=True)).to_have_count(0)
   expect(panel).to_contain_text('stays local')
   expect(output).to_have_value(machine)
   checks.append({'name':'Local suggestions are opt-in and keep cloud controls absent even when a cloud account is configured','passed':True})
   if a.check_context:
    context=panel.locator('[data-wording-context]')
    expect(context.locator('[data-wording-original]')).to_have_text(source)
    expect(context.locator('[data-wording-current]')).to_have_text(machine)
    for width,height in [(1280,960),(700,760),(520,760)]:
     page.set_viewport_size({'width':width,'height':height})
     wording.click();wording.click();page.wait_for_timeout(120)
     choices=panel.locator('[data-wording-choices]')
     choices.evaluate('(element)=>element.scrollTop=element.scrollHeight')
     box=context.bounding_box();assert box and box['y']>=0 and box['y']+box['height']<=height,(width,box)
     expect(context.locator('[data-wording-current]')).to_have_text(machine)
     page.screenshot(path=str(a.report.with_name(a.report.stem+f'-{width}.png')))
    page.set_viewport_size({'width':1280,'height':960})
    checks.append({'name':'Original and complete current result stay available above independently scrolling wording choices at desktop, narrow and phone-width views','passed':True})
   panel.get_by_role('button',name='Use this wording',exact=True).first.click()
   expect(output).to_have_value(revised)
   expect(page.locator('#style-trial-input')).to_have_value(source)
   if a.check_context:expect(panel.locator('[data-wording-current]')).to_have_text(revised)
   if a.check_boundaries:
    expect(panel.locator('[data-wording-preview]').first).to_have_text(machine.replace('Can I move','Would I be able to move'))
   expect(page.locator('[data-style-trial-retention]')).to_contain_text('Not saved')
   checks.append({'name':'Choosing a local request prefix preserves subject, request body, dates, condition and untouched literal model wording','passed':True})
   page.get_by_role('button',name='Save draft',exact=True).click()
   page.get_by_role('button',name='Open this saved draft',exact=True).click()
   saved=page.get_by_role('dialog',name='Saved drafts',exact=True)
   expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_value(revised)
   checks.append({'name':'Saved draft and original use the revised current result through the existing save/read flow','passed':True})
   if a.check_undo:
    saved.get_by_role('button',name='Close',exact=True).click()
    panel.get_by_role('button',name='Use this wording',exact=True).last.click()
    softer=machine.replace('Can I move','Would I be able to move')
    expect(output).to_have_value(softer)
    undo=panel.get_by_role('button',name='Undo wording change',exact=True)
    can_undo=undo.count()==1
    checks.append({'name':'Adopting a softer question shows a named receipt and explicit Undo instead of a misleading no-options state','passed':can_undo and panel.get_by_text('Applied: Softer question',exact=True).count()==1 and panel.get_by_text('No matching local request option.',exact=False).count()==0})
    if can_undo:
     undo.click();expect(output).to_have_value(revised)
     if a.check_context:expect(panel.locator('[data-wording-current]')).to_have_text(revised)
     expect(page.locator('#style-trial-input')).to_have_value(source)
     expect(page.locator('[data-style-trial-retention]')).to_contain_text('Saved only in this browser session')
     checks.append({'name':'Undo restores only the immediately preceding workspace text and its existing saved acknowledgement, without changing the original or saved record','passed':True})
     panel.get_by_role('button',name='Use this wording',exact=True).last.click()
     expect(output).to_have_value(softer)
     page.evaluate('(value)=>window.__wordingOutput=value',softer)
     page.get_by_role('button',name='Translate',exact=True).click()
     expect(output).to_have_value(softer)
     expect(page.get_by_role('button',name='Undo wording change',exact=True)).to_have_count(0)
     checks.append({'name':'A new translation result with identical text cannot borrow the previous result’s undo receipt','passed':True})
     output.fill(machine);page.get_by_role('button',name='Wording options',exact=True).click()
     panel.get_by_role('button',name='Use this wording',exact=True).first.click()
     expect(output).to_have_value(revised)
     manual=revised+' Review this manually.';output.fill(manual)
     if a.check_context:expect(panel.locator('[data-wording-current]')).to_have_text(manual)
     expect(page.get_by_role('button',name='Undo wording change',exact=True)).to_have_count(0)
     expect(output).to_have_value(manual)
     checks.append({'name':'Manual editing invalidates the stale undo action without overwriting the newer text','passed':True})
   if a.check_boundaries:
    compound="I've sent the modified file to your mailbox. If you can't open the attachment, tell me, I'll send it again."
    output.fill(compound)
    expect(panel.get_by_role('button',name='Use this wording',exact=True)).to_have_count(0)
    expect(panel.locator('[data-wording-preview]')).to_have_count(0)
    expect(panel.locator('[data-wording-current]')).to_have_text(compound)
    expect(output).to_have_value(compound)
    checks.append({'name':'Actual168 compound imperative is retained verbatim but no longer offered as two malformed questions','passed':True})
    complete='Could you send the revised file? I will send the link again.'
    output.fill(complete)
    expect(panel.locator('[data-wording-preview]').first).to_have_text(complete.replace('Could you send','Could you please send'))
    expect(panel.locator('[data-wording-preview]').last).to_have_text(complete.replace('Could you send','Would you be able to send'))
    expect(output).to_have_value(complete)
    checks.append({'name':'Supported request choices preview the entire proposed message with its unchanged following sentence before explicit adoption','passed':True})
  calls=page.evaluate('window.__wordingCalls.map(x=>x.cmd)')
  forbidden=[c for c in calls if c in ['suggest_polite_text_edits','try_translation_text','prepare_local_translation','set_credential'] or re.match(r'^(copy_|insert_)',c)]
  assert not forbidden,forbidden
  if available:
   saves=page.evaluate("window.__wordingCalls.filter(x=>x.cmd==='save_text_draft').map(x=>x.args)")
   assert saves[-1]['original']==source and saves[-1]['text']==revised
  checks.append({'name':'No cloud request, new preparation, credential, clipboard or external insertion is triggered','passed':True})
  if a.check_context:
   # Change only this isolated fixture's UI locale. Reload would run the
   # original English init script again and invalidate the controller premise.
   page.evaluate("async()=>{const {default:i18n}=await import('/src/i18n/index.ts');await i18n.changeLanguage('zh-CN')}")
   expect(page.locator('#style-trial-title')).to_have_text('翻译成英语')
   checks.append({'name':'Chinese translation heading localizes the target without changing its stored/native language value','passed':True})
 finally:b.close()
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':all(x['passed'] for x in checks),'checks':checks,'originalMachineText':machine,'expectedExplicitEdit':revised}
a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps(r,ensure_ascii=False))
if not r['passed'] and not a.capture_before:raise SystemExit(1)
