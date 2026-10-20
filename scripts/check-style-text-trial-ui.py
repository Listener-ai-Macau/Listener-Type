"""Exercise text-trial lifecycle in an isolated browser, without native/cloud writes."""
import argparse, datetime, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
p.add_argument('--screenshot', type=Path)
a = p.parse_args()
checks = []
def check(name, value):
    checks.append({'check': name, 'passed': bool(value)})
    assert value, name

fixture_js = """
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
window.__trialLog=[]; window.__trialOutcome='pending'; window.__trialResolvers=[];
window.__trialFixture=(cmd,args,mock)=>{
 window.__trialLog.push({cmd,args});
 if(cmd!=='try_style_pack_text') return mock();
 if(window.__trialOutcome==='preview') return mock();
 if(window.__trialOutcome==='reject') return Promise.reject(new Error('Sample service unavailable'));
 return new Promise(resolve=>window.__trialResolvers.push(()=>resolve({
  packId:args.stylePackId,packName:args.stylePackId,outputText:'Trial: '+args.rawText,usedTextService:args.stylePackId!=='builtin.raw',elapsedMs:1200
 })));
};
"""
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width':1280,'height':1000})
        page.set_default_timeout(6000)
        page.add_init_script(fixture_js)
        patched=[]
        def fixture(route):
            r=route.fetch();source=r.text()
            old='if (!isTauri) {\n    return mock();\n  }'
            assert source.count(old)==1
            patched.append(True)
            route.fulfill(response=r,body=source.replace(old,'if (!isTauri) {\n    return window.__trialFixture(cmd,args,mock);\n  }',1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
        page.goto(a.url,wait_until='networkidle')
        page.get_by_role('button',name='Style',exact=True).click()
        expect(page.get_by_role('button',name='Try 轻度润色',exact=True)).to_be_visible()
        # Exclude the existing app startup locale synchronization from trial actions.
        page.evaluate('window.__trialLog=[]')
        page.get_by_role('button',name='Try 轻度润色',exact=True).click()
        dialog=page.get_by_role('dialog')
        expect(dialog).to_be_visible()
        check('Opening a different style sends no processing or activation request',not any(x['cmd'] in ['try_style_pack_text','set_active_style_pack'] for x in page.evaluate('window.__trialLog')))
        run=dialog.get_by_role('button',name='Try this text',exact=True)
        expect(run).to_be_disabled()
        entry=dialog.get_by_label('Your text',exact=True)
        entry.fill('   ');expect(run).to_be_disabled()
        entry.fill('My own message, <b>without markup</b>.')
        run.click();expect(dialog.get_by_role('button',name='Processing…',exact=True)).to_be_disabled();expect(entry).to_be_disabled()
        requests=[x for x in page.evaluate('window.__trialLog') if x['cmd']=='try_style_pack_text']
        check('Pending trial sends one request for the explicit selected saved style',len(requests)==1 and requests[0]['args']['stylePackId']=='builtin.light')
        dialog.get_by_role('button',name='Close',exact=True).click()
        page.get_by_role('button',name='Try 原文',exact=True).click()
        expect(entry).to_have_value('My own message, <b>without markup</b>.')
        page.evaluate('window.__trialResolvers.shift()()')
        check('A late result cannot appear in a reopened trial for another style',dialog.get_by_role('region',name='Result').count()==0)
        page.evaluate("window.__trialOutcome='reject'");run.click()
        expect(dialog.get_by_role('alert')).to_contain_text('Sample service unavailable')
        expect(entry).to_have_value('My own message, <b>without markup</b>.')
        check('Failure preserves the input and enables retry',run.is_enabled())
        page.evaluate("window.__trialOutcome='pending'");run.click();page.evaluate('window.__trialResolvers.shift()()')
        output=dialog.get_by_role('region',name='Result')
        expect(output).to_contain_text('Trial: My own message, <b>without markup</b>.')
        check('Returned text is rendered as text, without executing markup',output.locator('b').count()==0)
        entry.fill('Changed draft')
        check('Editing the input clears the result for the previous text',output.count()==0)
        dialog.get_by_role('button',name='Text-service settings',exact=True).click()
        page.get_by_role('button',name='Close',exact=True).click()
        page.get_by_role('button',name='Try 原文',exact=True).click()
        expect(entry).to_have_value('Changed draft')
        check('Opening service settings preserves the in-memory draft',True)
        page.evaluate("window.__trialOutcome='preview'");run.click()
        expect(dialog.get_by_role('alert')).to_contain_text('This browser preview does not simulate a result')
        check('Ordinary preview never claims a real service result',output.count()==0)
        log=page.evaluate('window.__trialLog')
        forbidden=['set_active_style_pack','set_settings','delete_history','clear_history','repolish','hotkey_recording_toggle','start_recording']
        check('Trial does not activate a style, save preferences or touch dictation/history',not any(x['cmd'] in forbidden for x in log))
        if a.screenshot:page.screenshot(path=str(a.screenshot),full_page=True)
        page.keyboard.press('Escape');expect(dialog).to_have_count(0)
        check('Native dialog supports keyboard dismissal',True)
        check('Only the isolated browser was instrumented',bool(patched))
    finally: browser.close()
report={'at':datetime.datetime.now().astimezone().isoformat(),'url':a.url,'scope':'Isolated headless browser; controlled results are test fixtures, not cloud or customer success. No daily Type, native credentials, clipboard, recording or hardware touched.','checks':checks,'passed':all(x['passed'] for x in checks)}
a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))
