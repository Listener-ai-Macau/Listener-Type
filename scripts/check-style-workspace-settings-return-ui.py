"""Settings detours must not discard a text workspace or its in-flight result.

Production React with a controlled IPC fixture in a headless browser. No native
credentials, cloud calls, clipboard, hardware, or user desktop interaction.
"""
import argparse
import json
import re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', required=True, type=Path)
p.add_argument('--expect-loss', action='store_true')
a = p.parse_args()
report = {'scope': __doc__, 'passed': False, 'cases': []}
fixture_js = """
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.__returnCalls=[]; window.__returnReady=true;
window.__returnFixture=(cmd,args,mock)=>{
 window.__returnCalls.push({cmd,args});
 if(cmd==='get_style_text_trial_readiness') return {
  available:true,requiresTextService:args.stylePackId!=='builtin.raw',
  textServiceConfigured:window.__returnReady
 };
 if(cmd==='try_style_pack_text') return new Promise(resolve=>{
  window.__finishReturnTrial=()=>resolve({packId:args.stylePackId,
   packName:args.stylePackId,outputText:'Processed: '+args.rawText,
   usedTextService:true,elapsedMs:900});
 });
 return mock();
};
"""
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge')
    try:
        for entry, pending in [('overview', False), ('style', False), ('overview', True)]:
            context = browser.new_context(viewport={'width': 1280, 'height': 1000})
            context.route(re.compile(r'^https?://'), lambda r: r.continue_()
                if re.match(r'^https?://(localhost|127\.0\.0\.1)(:|/)', r.request.url) else r.abort())
            page = context.new_page()
            page.set_default_timeout(6500)
            page.add_init_script(fixture_js)
            def fixture(route):
                response = route.fetch()
                source = response.text()
                old = 'if (!isTauri) {\n    return mock();\n  }'
                assert source.count(old) == 1
                route.fulfill(response=response, body=source.replace(old,
                    'if (!isTauri) {\n    return window.__returnFixture(cmd,args,mock);\n  }', 1))
            page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), fixture)
            page.goto(a.url, wait_until='networkidle')
            if entry == 'overview':
                page.get_by_role('button', name='Try typed text', exact=True).click()
            else:
                page.get_by_role('button', name='Style', exact=True).click()
                page.get_by_role('button', name='Try Light polish', exact=True).click()
            workspace = page.locator('dialog.ol-style-trial').first
            workspace.get_by_role('combobox', name='Trial style', exact=True).select_option('builtin.light')
            original = 'Keep Thursday at three, unless the room is unavailable. 条件😀'
            reviewed = 'My reviewed wording — Thursday at three; room condition unchanged. 条件😀'
            workspace.get_by_label('Your text', exact=True).fill(original)
            workspace.get_by_role('button', name='Try this text', exact=True).click()
            page.wait_for_function('!!window.__finishReturnTrial')
            if not pending:
                page.evaluate('window.__finishReturnTrial()')
                expect(workspace.get_by_label('Result text', exact=True)).to_have_value('Processed: '+original)
                workspace.get_by_label('Result text', exact=True).fill(reviewed)
            before = page.evaluate('window.__returnCalls.filter(x=>x.cmd==="get_style_text_trial_readiness").length')
            workspace.get_by_role('button', name='Text-service settings', exact=True).click()
            if a.expect_loss:
                expect(page.locator('textarea[aria-label="Your text"]')).to_have_count(0)
                if pending:
                    page.evaluate('window.__finishReturnTrial()')
                report['cases'].append({'entry':entry,'pending':pending,'workspaceUnmounted':True,
                    'reviewedResultRetained':False,'trialRequests':1})
            else:
                settings = page.get_by_role('dialog', name='Text-service settings', exact=True)
                expect(settings).to_be_visible()
                assert workspace.count() == 1
                assert workspace.get_by_label('Your text', exact=True).input_value() == original
                if pending:
                    page.evaluate('window.__finishReturnTrial()')
                expect(workspace.get_by_label('Result text', exact=True)).to_have_value('Processed: '+original if pending else reviewed)
                settings.get_by_role('button', name='Close', exact=True).click()
                expect(settings).not_to_be_visible()
                expect(workspace.get_by_label('Your text', exact=True)).to_have_value(original)
                expect(workspace.get_by_label('Result text', exact=True)).to_have_value('Processed: '+original if pending else reviewed)
                assert workspace.get_by_role('combobox').input_value() == 'builtin.light'
                page.wait_for_function('(n)=>window.__returnCalls.filter(x=>x.cmd==="get_style_text_trial_readiness").length>n', arg=before)
                report['cases'].append({'entry':entry,'pending':pending,'workspaceUnmounted':False,
                    'originalAndSelectedStyleRetained':True,'reviewedResultRetained':True,
                    'readinessRecheckedAfterReturn':True})
            calls = page.evaluate('window.__returnCalls')
            assert sum(c['cmd']=='try_style_pack_text' for c in calls) == 1
            assert not [c for c in calls if re.match(r'^(set_credential|set_active_|update_preferences|copy_|insert_|start_dictation)',c['cmd'])]
            report['cases'][-1]['noExtraProcessingOrPersistentChanges'] = True
            context.close()
        report['passed'] = True
    except Exception as e:
        report['error'] = repr(e)
        raise
    finally:
        a.report.parent.mkdir(parents=True, exist_ok=True)
        a.report.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
        browser.close()
print(json.dumps({'passed': report['passed'], 'cases': report['cases']}, ensure_ascii=False))
