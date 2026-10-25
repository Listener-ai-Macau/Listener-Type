"""First-use directions must match the current input source, not a universal key.

Real React/i18n with controlled preference replies. No device, speech, global
key, Owner desktop/clipboard, model or cloud qualification.
"""
import argparse, json, re
from pathlib import Path
from playwright.sync_api import expect, sync_playwright

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
a = p.parse_args()
report = {'scope': __doc__, 'passed': False, 'cases': []}
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge')
    try:
        for locale in ['en','zh-CN','zh-TW','ja','ko']:
            for source in ['embeddedBle','microphone']:
                context = browser.new_context(viewport={'width':1280,'height':1000})
                context.route(re.compile(r'^https?://'), lambda r: r.continue_() if re.match(r'^https?://(127\.0\.0\.1|localhost)(:|/)',r.request.url) else r.abort())
                page = context.new_page()
                page.set_default_timeout(6500)
                page.add_init_script('''
                  localStorage.setItem('ol.locale',LOCALE);
                  sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
                  sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
                  window.__guideCalls=[];
                  window.__guideFixture=(cmd,args,mock)=>{
                    window.__guideCalls.push(cmd);const value=mock();
                    return cmd==='get_settings'?{...value,dictationInputSource:SOURCE,
                      dictationHotkey:{primary:'F8',modifiers:['ctrl']}}:value;
                  };
                '''.replace('LOCALE',json.dumps(locale)).replace('SOURCE',json.dumps(source)))
                def fixture(route):
                    response = route.fetch()
                    code = response.text()
                    old = 'if (!isTauri) {\n    return mock();\n  }'
                    assert code.count(old)==1
                    route.fulfill(response=response,body=code.replace(old,'if (!isTauri) {\n    return window.__guideFixture(cmd,args,mock);\n  }',1))
                page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
                page.goto(a.url,wait_until='networkidle')
                guide = page.locator('.ol-overview-start ol')
                expect(guide.locator('li')).to_have_count(3)
                if source=='embeddedBle':
                    expect(guide).to_contain_text('Listener')
                    expect(guide).not_to_contain_text('F8')
                else:
                    expect(guide).to_contain_text('F8')
                    expect(guide).not_to_contain_text('Listener')
                assert 'overview.firstDictation' not in guide.inner_text()
                calls=page.evaluate('window.__guideCalls')
                assert not any(cmd in calls for cmd in ['start_dictation','set_selected_device_settings','set_selected_device_key_profile'])
                report['cases'].append({'locale':locale,'source':source,'instructions':guide.inner_text(),'configuredShortcutOnlyForMicrophone':True,'browserOnlyLocalePreferenceInitializations':calls.count('set_settings')})
                if locale=='zh-CN': guide.screenshot(path=str(a.report.with_name(a.report.stem+'-'+source+'.png')))
                context.close()
        report['passed'] = True
    finally:
        browser.close()
        a.report.parent.mkdir(parents=True,exist_ok=True)
        a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
print(json.dumps({'passed':report['passed'],'cases':len(report['cases']),'scope':report['scope']},ensure_ascii=False))
