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
window.__copyLog=[];window.__copyResolve=null;window.__copyReject=null;
Object.defineProperty(navigator,'clipboard',{value:{writeText:text=>{window.__copyLog.push(text);return new Promise((resolve,reject)=>{window.__copyResolve=resolve;window.__copyReject=reject;});}}});
window.__styleListFail=false; window.__trialLog=[]; window.__trialOutcome='pending'; window.__trialResolvers=[]; window.__trialConfigured=true;
window.__addOutcome='reject'; window.__addResolve=null;
window.__trialFixture=(cmd,args,mock)=>{
 window.__trialLog.push({cmd,args});
 if(cmd==='list_style_packs' && window.__styleListFail) return Promise.reject(new Error('saved styles unavailable'));
 if(cmd==='get_style_text_trial_readiness') return {available:window.__trialOutcome!=='preview',requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:window.__trialConfigured};
 if(cmd==='create_style_pack_from_template') {
  if(window.__addOutcome==='reject') return Promise.reject(new Error('save unavailable'));
  if(window.__addOutcome==='pending') return new Promise(resolve=>window.__addResolve=()=>resolve(mock()));
  return mock();
 }
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
        page.evaluate('window.__styleListFail=true')
        page.get_by_role('button',name='Try typed text',exact=True).click()
        expect(page.get_by_role('dialog').get_by_role('alert')).to_contain_text('Could not load your saved styles')
        page.evaluate('window.__styleListFail=false')
        page.get_by_role('dialog').get_by_role('button',name='Retry',exact=True).click()
        quick=page.get_by_role('dialog')
        expect(quick).to_be_visible()
        expect(quick.get_by_label('Your text',exact=True)).to_be_visible()
        check('Overview opens the draft directly without visiting style management or processing',not page.get_by_role('button',name='Import ZIP',exact=True).is_visible() and not any(x['cmd'] in ['try_style_pack_text','set_active_style_pack','start_dictation'] for x in page.evaluate('window.__trialLog')))
        quick.get_by_label('Your text',exact=True).fill('Keep my draft while choosing a style')
        quick.get_by_role('combobox',name='Trial style').select_option('builtin.formal')
        expect(quick.get_by_label('Your text',exact=True)).to_have_value('Keep my draft while choosing a style')
        check('Choosing a trial style preserves draft without changing the active style or sending text',not any(x['cmd'] in ['try_style_pack_text','set_active_style_pack'] for x in page.evaluate('window.__trialLog')))
        quick.get_by_role('combobox',name='Trial style').select_option('builtin.structured')
        # This browser fixture carries a saved non-factory description; display it verbatim.
        expect(quick.locator('[data-style-trial-purpose]')).to_have_text('适合多事项和多主题口述，自动整理为层次清楚的结构化输出。')
        quick.get_by_role('button',name='Add Everyday list for messages',exact=True).click()
        expect(quick.get_by_role('alert')).to_contain_text('Could not add the style')
        expect(quick.get_by_label('Your text',exact=True)).to_have_value('Keep my draft while choosing a style')
        check('A failed optional style save keeps the existing selection and draft, and offers retry',quick.get_by_role('combobox').input_value()=='builtin.structured')
        page.evaluate("window.__addOutcome='pending'")
        quick.get_by_role('button',name='Add Everyday list for messages',exact=True).click()
        expect(quick.get_by_role('button',name='Adding style…',exact=True)).to_be_disabled()
        expect(quick.get_by_role('combobox')).to_be_disabled()
        expect(quick.get_by_role('button',name='Try this text',exact=True)).to_be_disabled()
        page.evaluate('window.__addResolve()')
        expect(quick.get_by_role('heading')).to_have_text('Try Everyday list with your text')
        expect(quick.get_by_label('Your text',exact=True)).to_have_value('Keep my draft while choosing a style')
        created=[x for x in page.evaluate('window.__trialLog') if x['cmd']=='create_style_pack_from_template'][-1]['args']['template']
        import hashlib
        check('Optional everyday style saves the tested independent prompt without making it active',created['kind']=='imported' and created['baseMode']=='structured' and not created['active'] and hashlib.sha256(created['prompt'].encode()).hexdigest()=='999c281df2b68ad8c2594a1290da3c3cd49d70f1d8313550f7816664c8363103')
        check('Adding a style does not process text or change recording defaults',not any(x['cmd'] in ['try_style_pack_text','set_active_style_pack','save_style_pack'] for x in page.evaluate('window.__trialLog')))
        quick.get_by_role('combobox').select_option('builtin.structured')
        quick.get_by_role('button',name='Choose saved Everyday list',exact=True).click()
        expect(quick.get_by_role('heading')).to_have_text('Try Everyday list with your text')
        check('Choosing the saved optional style reuses it rather than adding a duplicate',len([x for x in page.evaluate('window.__trialLog') if x['cmd']=='create_style_pack_from_template'])==2)
        quick.get_by_role('button',name='Close',exact=True).click()
        page.get_by_role('button',name='Try typed text',exact=True).click()
        expect(quick.get_by_label('Your text',exact=True)).to_have_value('Keep my draft while choosing a style')
        check('Closing and reopening Overview trial preserves its in-memory draft',True)
        quick.get_by_role('button',name='Close',exact=True).click()
        page.get_by_role('button',name='Style',exact=True).click()
        expect(page.get_by_role('button',name='Try Light polish',exact=True)).to_be_visible()
        # Exclude the existing app startup locale synchronization from trial actions.
        page.evaluate('window.__trialLog=[]')
        page.get_by_role('button',name='Try Light polish',exact=True).click()
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
        page.get_by_role('button',name='Try Original text',exact=True).click()
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
        page.set_viewport_size({'width':1280,'height':800})
        check('Returned body sits beside the draft with its copy action in view',output.bounding_box()['x']>entry.bounding_box()['x']+entry.bounding_box()['width'] and output.get_by_role('button',name='Copy result',exact=True).is_visible())
        expect(dialog.locator('.ol-style-trial-details')).not_to_have_attribute('open','')
        check('Compact cost and text-only notice stays visible before the draft while full details remain available',dialog.get_by_text('Text-only trial, not speech recognition.',exact=False).is_visible() and dialog.locator('.ol-style-trial-details summary').is_visible())
        if a.screenshot:page.screenshot(path=str(a.screenshot),full_page=True)
        copy=dialog.get_by_role('button',name='Copy result',exact=True)
        copy.click()
        expect(dialog.get_by_role('button',name='Copying…',exact=True)).to_be_disabled()
        expect(entry).to_be_disabled()
        check('Explicit Copy captures only the displayed actual result, not its label or draft',page.evaluate('window.__copyLog')==['Trial: My own message, <b>without markup</b>.'])
        check('Pending clipboard write does not claim copied',dialog.get_by_role('button',name='Copied',exact=True).count()==0)
        page.evaluate('window.__copyResolve()')
        expect(dialog.get_by_role('button',name='Copied',exact=True)).to_be_enabled()
        expect(entry).to_be_enabled()
        entry.fill('Changed draft')
        check('Editing the input clears the result for the previous text',output.count()==0)
        check('Editing clears the old Copy acknowledgement',dialog.get_by_role('button',name='Copied',exact=True).count()==0)
        run.click();page.evaluate('window.__trialResolvers.shift()()')
        expect(output).to_contain_text('Trial: Changed draft')
        dialog.get_by_role('button',name='Copy result',exact=True).click()
        page.evaluate('window.__copyReject(new Error("clipboard denied"))')
        expect(dialog.get_by_role('alert')).to_contain_text('Could not copy the result')
        check('Copy failure retains result and allows an explicit retry',copy.is_enabled() and 'Trial: Changed draft' in output.inner_text())
        copy.click()
        dialog.get_by_role('button',name='Close',exact=True).click()
        page.get_by_role('button',name='Try Original text',exact=True).click()
        page.evaluate('window.__copyResolve()')
        check('Closed-dialog copy acknowledgement cannot appear in a newly opened trial',dialog.get_by_role('button',name='Copied',exact=True).count()==0)
        dialog.get_by_role('button',name='Text-service settings',exact=True).click()
        page.get_by_role('button',name='Close',exact=True).click()
        page.get_by_role('button',name='Try Original text',exact=True).click()
        expect(entry).to_have_value('Changed draft')
        check('Opening service settings preserves the in-memory draft',True)
        dialog.get_by_role('button',name='Close',exact=True).click()
        page.evaluate('window.__trialConfigured=false')
        page.get_by_role('button',name='Try Light polish',exact=True).click()
        expect(dialog.get_by_role('status')).to_contain_text('This style needs a text service')
        expect(run).to_be_disabled()
        check('Missing text service is disclosed before submission without replacing input',entry.input_value()=='Changed draft')
        dialog.get_by_role('button',name='Text-service settings',exact=True).click()
        llm=page.get_by_text('LLM (polishing)',exact=True)
        asr=page.get_by_text('ASR (transcription)',exact=True)
        expect(llm).to_be_visible()
        check('The text-service action puts its LLM card first in both reading and visual order',llm.bounding_box()['y']<asr.bounding_box()['y'] and llm.evaluate('(a,b)=>!!(a.compareDocumentPosition(b)&Node.DOCUMENT_POSITION_FOLLOWING)',asr.element_handle()))
        llm_card=llm.locator('..').locator('..').element_handle()
        page.get_by_role('button',name='Providers',exact=True).click()
        check('Normal Providers navigation keeps the existing speech-first order',asr.bounding_box()['y']<llm.bounding_box()['y'])
        check('Reordering preserves the existing credential-card component identity',llm_card.evaluate('(el)=>el.isConnected'))
        page.get_by_role('button',name='Close',exact=True).click()
        page.evaluate("window.__trialOutcome='preview'")
        page.get_by_role('button',name='Try Original text',exact=True).click()
        expect(dialog.get_by_role('status')).to_contain_text('This browser preview does not simulate a result')
        expect(entry).to_be_enabled();expect(run).to_be_disabled()
        check('Ordinary preview allows drafts but discloses its limit and cannot submit',output.count()==0)
        log=page.evaluate('window.__trialLog')
        forbidden=['set_active_style_pack','set_settings','delete_history','clear_history','repolish','hotkey_recording_toggle','start_recording']
        check('Trial does not activate a style, save preferences or touch dictation/history',not any(x['cmd'] in forbidden for x in log))
        if a.screenshot:page.screenshot(path=str(a.screenshot.with_name(a.screenshot.stem+'-unavailable'+a.screenshot.suffix)),full_page=True)
        page.keyboard.press('Escape');expect(dialog).to_have_count(0)
        check('Native dialog supports keyboard dismissal',True)
        check('Only the isolated browser was instrumented',bool(patched))
    finally: browser.close()
report={'at':datetime.datetime.now().astimezone().isoformat(),'url':a.url,'scope':'Isolated headless browser; controlled results are test fixtures, not cloud or customer success. No daily Type, native credentials, clipboard, recording or hardware touched.','checks':checks,'passed':all(x['passed'] for x in checks)}
a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))
