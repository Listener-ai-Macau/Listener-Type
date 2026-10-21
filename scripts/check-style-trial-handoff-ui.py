"""Owned headless UI: explicit handoff, cancellation, stale completion and honest status."""
import argparse, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True)
a=p.parse_args();checks=[]
with sync_playwright() as pw:
    browser=pw.chromium.launch(headless=True,channel='msedge')
    try:
        page=browser.new_page(viewport={'width':1280,'height':1000})
        page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
window.__handoffs=[];window.__cancels=[];
window.__trialFixture=(cmd,args,mock)=>{
if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:false,textServiceConfigured:true};
if(cmd==='try_style_pack_text')return {packId:args.stylePackId,packName:'Original text',outputText:args.rawText,usedTextService:false,elapsedMs:0};
if(cmd==='insert_style_trial_result')return new Promise((resolve,reject)=>window.__handoffs.push({args,resolve,reject}));
if(cmd==='cancel_style_trial_insert'){window.__cancels.push(args.requestId);return true;}
return mock();};""")
        def fixture(route):
            response=route.fetch();s=response.text();old='if (!isTauri) {\n    return mock();\n  }'
            assert s.count(old)==1
            s=s.replace(old,'if (!isTauri) {\n    return window.__trialFixture(cmd,args,mock);\n  }',1)
            old='export const canInsertStyleTrialResult = isTauri && /Windows/i.test(navigator.userAgent);'
            assert s.count(old)==1
            route.fulfill(response=response,body=s.replace(old,'export const canInsertStyleTrialResult = true;',1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
        page.goto(a.url,wait_until='networkidle')
        page.get_by_role('button',name='Try typed text',exact=True).click()
        dialog=page.get_by_role('dialog');entry=dialog.get_by_label('Your text',exact=True)
        entry.fill('Keep all requirements. 同名文件不要覆盖。 😀')
        dialog.get_by_role('button',name='Try this text',exact=True).click()
        insert=dialog.get_by_role('button',name='Insert in another app…',exact=True)
        expect(insert).to_be_visible()
        assert page.evaluate('window.__handoffs.length')==0
        checks.append('processing a draft never starts insertion automatically')
        insert.click();expect(entry).to_be_disabled();expect(insert).to_be_disabled()
        expect(dialog.get_by_role('status')).to_contain_text('Click your destination text field now')
        assert page.evaluate('window.__handoffs[0].args.text')=='Keep all requirements. 同名文件不要覆盖。 😀'
        checks.append('explicit click binds exact reviewed Unicode result and disables conflicting edits')
        dialog.get_by_role('button',name='Cancel',exact=True).click();expect(entry).to_be_enabled()
        page.evaluate("window.__handoffs[0].resolve('confirmed')")
        expect(dialog.get_by_role('status')).to_have_text('Insertion cancelled. Your result stays here.')
        checks.append('cancelled earlier request cannot publish late success over current state')
        insert.click();page.evaluate("window.__handoffs[1].resolve('sent')")
        expect(dialog.get_by_role('status')).to_have_text('Paste was sent. Check the destination; on-screen text was not confirmed.')
        checks.append('unconfirmed paste is never labelled confirmed insertion')
        insert.click();dialog.get_by_role('button',name='Close',exact=True).click()
        expect(dialog).not_to_be_visible()
        page.wait_for_function('window.__cancels.length===2')
        checks.append('closing the workspace cancels the still-pending handoff')
    finally:browser.close()
a.report.parent.mkdir(parents=True,exist_ok=True)
a.report.write_text(json.dumps({'scope':'Private headless frontend with explicit mock handoff; no native focus, clipboard, service or hardware action.','checks':checks,'passed':len(checks)==5},ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps({'checks':len(checks),'passed':True}))
