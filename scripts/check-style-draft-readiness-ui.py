"""Ordinary input during delayed/failed service readiness, in owned headless UI only."""
import argparse, datetime, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
p.add_argument('--before', action='store_true')
a = p.parse_args()
report = {'at': datetime.datetime.now().astimezone().isoformat(), 'scope': 'Controlled readiness delay/failure in an owned headless frontend; ordinary keyboard input, no cloud/native/hardware or customer acceptance.', 'before': a.before}
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width':1280,'height':900})
        page.add_init_script("""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
window.__draftCalls=[]; window.__readyResolve=null; window.__readyReject=null;
window.__draftFixture=(cmd,args,mock)=>{
 window.__draftCalls.push({cmd,args});
 if(cmd==='get_style_text_trial_readiness') return new Promise((resolve,reject)=>{window.__readyResolve=resolve;window.__readyReject=reject;});
 return mock();
};
""")
        def fixture(route):
            response = route.fetch()
            source = response.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert source.count(old) == 1
            route.fulfill(response=response, body=source.replace(old, 'if (!isTauri) {\n    return window.__draftFixture(cmd,args,mock);\n  }', 1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), fixture)
        page.goto(a.url, wait_until='networkidle')
        # Interface-language synchronization belongs to startup, before this interaction.
        report['startupCommands'] = page.evaluate('window.__draftCalls.map(x=>x.cmd)')
        page.evaluate('window.__draftCalls=[]')
        page.get_by_role('button', name='Try typed text', exact=True).click()
        dialog = page.get_by_role('dialog')
        entry = dialog.get_by_label('Your text', exact=True)
        run = dialog.get_by_role('button', name=re.compile(r'^(?:Try this text|Process text|Generate draft|Preparing…)$'), exact=True)
        expect(dialog.get_by_role('status')).to_contain_text('Checking trial availability')
        expect(run).to_be_disabled()
        rect = entry.bounding_box()
        page.mouse.click(rect['x']+20,rect['y']+20)
        payload = 'Can we meet at 3? Keep Friday as the alternative.'
        page.keyboard.insert_text(payload)
        report['disabledWhileChecking'] = entry.is_disabled()
        report['textWhileChecking'] = entry.input_value()
        if a.before:
            assert report['disabledWhileChecking'] and report['textWhileChecking'] == ''
            report['reproducedDiscardedInput'] = True
        else:
            assert not report['disabledWhileChecking'] and report['textWhileChecking'] == payload
            page.evaluate('window.__readyReject(new Error("readiness unavailable"))')
            expect(dialog.get_by_role('alert')).to_contain_text('Could not check')
            page.keyboard.insert_text(' Still editing.')
            expect(entry).to_have_value(payload+' Still editing.')
            expect(run).to_be_disabled()
            report['failedReadinessRetainsFocusAndAllowsDraft'] = True
            dialog.get_by_role('button', name='Retry', exact=True).click()
            expect(dialog.get_by_role('status')).to_contain_text('Checking trial availability')
            entry.click()
            page.keyboard.press('Control+End')
            page.keyboard.insert_text(' Retry is separate.')
            expect(entry).to_have_value(payload+' Still editing. Retry is separate.')
            page.evaluate('window.__readyResolve({available:true,requiresTextService:false,textServiceConfigured:false})')
            expect(run).to_be_enabled()
            report['readyRecoveryPreservesDraft'] = entry.input_value()
            report['interactionCommands'] = page.evaluate('window.__draftCalls.map(x=>x.cmd)')
            report['noProcessingActivationOrSettingsWrite'] = not any(x in ('try_style_pack_text','set_active_style_pack','set_settings') for x in report['interactionCommands'])
            assert report['noProcessingActivationOrSettingsWrite']
        report['passed'] = True
    except Exception as error:
        report.update(passed=False, error=str(error))
    finally:
        browser.close()
a.report.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
print(json.dumps(report, ensure_ascii=False))
raise SystemExit(0 if report['passed'] else 1)
