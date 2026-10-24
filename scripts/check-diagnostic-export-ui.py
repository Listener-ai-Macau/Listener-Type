"""Actual About/shared IPC lifecycle, with controlled OS-save/native outcomes.
No actual system dialog, archive, device, user profile, clipboard or service.
"""
import argparse, datetime, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
a = p.parse_args()
r = {'at': datetime.datetime.now().astimezone().isoformat(), 'scope': __doc__, 'passed': False, 'cases': []}
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width': 1280, 'height': 900})
        page.set_default_timeout(7000)
        page.add_init_script("""
if(!localStorage.getItem('ol.locale'))localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.diagOps=[];
window.diagnosticExternalOperation=async name=>{
 const op={name,nativeStarted:false}; window.diagOps.push(op);
 const target=await new Promise(resolve=>op.save=resolve);
 if(target===null)return null;
 op.nativeStarted=true;
 return new Promise((resolve,reject)=>{op.complete=resolve;op.fail=reject;});
};
""")

        def ipc(route):
            response = route.fetch()
            text = response.text()
            old = 'if (!isTauri) {\n    return `~/Downloads/${suggestedFileName}`;\n  }'
            start = text.index('async function performDiagnosticExport(')
            before, body = text[:start], text[start:]
            assert body.count(old) == 1, 'Instrument only the diagnostic external save/export boundary'
            body = body.replace(old, 'if (!isTauri) {\n    return window.diagnosticExternalOperation(suggestedFileName);\n  }', 1)
            text = before + body
            route.fulfill(response=response, body=text + '\nwindow.__diagnosticUnderTest={exportDiagnosticPackage,getDiagnosticExportSnapshot};\n')

        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), ipc)
        page.goto(a.url, wait_until='networkidle')
        page.locator('button[title=Settings]').click()
        page.get_by_role('button', name='About', exact=True).click()
        control = page.locator('[data-diagnostic-export-control]')
        button = control.locator('button')
        button.click()
        page.wait_for_function('window.diagOps.length===1')
        expect(button).to_be_disabled()
        joined = page.evaluate("""()=>{const m=window.__diagnosticUnderTest;
 const x=m.exportDiagnosticPackage('second.zip'); const y=m.exportDiagnosticPackage('third.zip');
 void x.catch(()=>{}); void y.catch(()=>{}); return x===y;}""")
        assert joined and page.evaluate('window.diagOps.length===1')
        page.get_by_role('button', name='Personalize', exact=True).click()
        page.get_by_role('button', name='About', exact=True).click()
        expect(button).to_be_disabled()
        assert page.evaluate('window.diagOps.length===1&&!window.diagOps[0].nativeStarted')
        r['cases'].append('Shared ownership starts before save dialog; concurrent entries and About reopen join one pending request')
        page.evaluate('window.diagOps[0].save("fixture-first.zip")')
        page.wait_for_function('window.diagOps[0].nativeStarted')
        page.get_by_role('button', name='Personalize', exact=True).click()
        page.evaluate('window.diagOps[0].complete("fixture-first.zip")')
        page.get_by_role('button', name='About', exact=True).click()
        expect(button).to_be_enabled()
        expect(control.locator('span[title="fixture-first.zip"]')).to_have_text('Saved')
        r['cases'].append('Completion while About is closed is retained on reopening without a second export')
        button.click()
        page.wait_for_function('window.diagOps.length===2')
        # The first success timer must not clear a later request's busy state.
        page.wait_for_timeout(4100)
        expect(button).to_be_disabled()
        page.evaluate('window.diagOps[1].save(null)')
        expect(button).to_be_enabled()
        assert not page.evaluate('window.diagOps[1].nativeStarted')
        expect(control.locator('span[title]')).to_have_count(0)
        r['cases'].append('Old success expiry cannot clear a new operation; cancelled save releases without native work or false success')
        button.click()
        page.wait_for_function('window.diagOps.length===3')
        page.evaluate('window.diagOps[2].save("fixture-error.zip")')
        page.wait_for_function('window.diagOps[2].nativeStarted')
        page.evaluate('window.diagOps[2].fail("fixture-native-export-failed")')
        expect(button).to_be_enabled()
        expect(control.locator('span[title="fixture-native-export-failed"]')).to_have_text('Export failed')
        page.get_by_role('button', name='Personalize', exact=True).click()
        page.get_by_role('button', name='About', exact=True).click()
        expect(control.locator('span[title="fixture-native-export-failed"]')).to_be_visible()
        button.click()
        page.wait_for_function('window.diagOps.length===4')
        page.evaluate('window.diagOps[3].save("fixture-retry.zip")')
        page.wait_for_function('window.diagOps[3].nativeStarted')
        page.evaluate('window.diagOps[3].complete("fixture-retry.zip")')
        expect(control.locator('span[title="fixture-retry.zip"]')).to_have_text('Saved')
        r['cases'].append('Native failure persists through navigation and an explicit retry replaces it with its own success')
        for locale, title in [('en', 'About'), ('zh-CN', '关于'), ('zh-TW', '關於'), ('ja', '情報'), ('ko', '정보')]:
            page.evaluate('(locale)=>localStorage.setItem("ol.locale",locale)', locale)
            page.reload(wait_until='networkidle')
            page.locator('button[title]').first.click()
            page.get_by_role('button', name=title, exact=True).click()
            button.click()
            page.wait_for_function('window.diagOps.length===1')
            page.evaluate('window.diagOps[0].save("fixture-busy.zip")')
            page.wait_for_function('window.diagOps[0].nativeStarted')
            page.evaluate('window.diagOps[0].fail("diagnostic_export_busy")')
            expect(button).to_be_enabled()
            message = control.locator('span[title]').get_attribute('title')
            assert message and 'diagnostic_export_busy' not in message and 'modal.about.' not in message
            r['cases'].append('Native admission conflict has translated feedback in ' + locale)
        r['passed'] = True
    except Exception as error:
        r.update(errorType=type(error).__name__, error=str(error))
        raise
    finally:
        try:
            browser.close()
        except Exception as error:
            r.update(passed=False, cleanupError=str(error))
        finally:
            r['finishedAt'] = datetime.datetime.now().astimezone().isoformat()
            a.report.parent.mkdir(parents=True, exist_ok=True)
            a.report.write_text(json.dumps(r, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
print(json.dumps(r, ensure_ascii=False))
