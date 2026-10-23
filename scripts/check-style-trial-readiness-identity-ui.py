"""Check selected-style privacy/eligibility at the actual React transition.

Owned headless UI with delayed readiness only. No real service, user data,
hardware or native acceptance is implied by these controlled observations.
"""
import argparse
from datetime import datetime
import json
from pathlib import Path
import re
import subprocess
import uuid

from playwright.sync_api import expect, sync_playwright

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
p.add_argument('--before', action='store_true')
p.add_argument('--before-ref', help='Read-only Git source for the original component; no checkout/reset.')
a = p.parse_args()
original_component = None
if a.before_ref:
    assert a.before
    root = Path(__file__).resolve().parent.parent
    source = subprocess.run(['git', 'show', a.before_ref+':src/components/StyleTextTrial.tsx'],
                            cwd=root, capture_output=True, check=True).stdout
    original_component = root/'src/components'/('.style-readiness-counterexample-'+uuid.uuid4().hex+'.tsx')
    original_component.write_bytes(source)
report = {'at': datetime.now().astimezone().isoformat(), 'before': a.before,
          'beforeRef': a.before_ref,
          'scope': 'Actual React render transition under controlled delayed readiness; not a customer-visible duration or real network route claim.'}
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width': 1280, 'height': 900})
        page.set_default_timeout(6000)
        page.add_init_script("""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
window.__calls=[]; window.__pending=[]; window.__transitions=[];
window.__fixture=(cmd,args,mock)=>{
  window.__calls.push(cmd);
  if(cmd!=='get_style_text_trial_readiness') return mock();
  const dialog=document.querySelector('dialog[aria-labelledby="style-trial-title"]');
  if(dialog)window.__transitions.push({requested:args.stylePackId,
    selected:dialog.querySelector('select')?.value,
    notice:dialog.querySelector('[data-style-trial-service-notice]')?.textContent,
    submitEnabled:![...dialog.querySelectorAll('button')].find(b=>b.textContent==='Try this text')?.disabled});
  if(!window.__delay)return {available:true,requiresTextService:false,textServiceConfigured:false};
  return new Promise((resolve,reject)=>window.__pending.push({id:args.stylePackId,resolve,reject}));
};
""")
        def fixture(route):
            r = route.fetch()
            source = r.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert source.count(old) == 1
            route.fulfill(response=r, body=source.replace(old, 'if (!isTauri) {\n    return window.__fixture(cmd,args,mock);\n  }', 1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), fixture)
        if original_component:
            def original(route):
                # Vite transforms the frozen source in the same directory, so
                # its normal imports/runtime are unchanged. Product files stay put.
                r = route.fetch(url=a.url+'/src/components/'+original_component.name)
                route.fulfill(response=r)
            page.route(re.compile(r'/src/components/StyleTextTrial\.tsx(?:\?.*)?$'), original)
        page.goto(a.url, wait_until='networkidle')
        page.get_by_role('button', name='Try typed text', exact=True).click()
        dialog = page.locator('dialog[aria-labelledby="style-trial-title"]')
        draft = dialog.get_by_label('Your text', exact=True)
        run = dialog.get_by_role('button', name='Try this text', exact=True)
        draft.fill('A local draft must not imply that a different style is local.')
        expect(run).to_be_enabled()
        raw_notice = dialog.locator('[data-style-trial-service-notice]').inner_text()
        page.evaluate('window.__delay=true; window.__transitions=[]')
        dialog.get_by_role('combobox').select_option('builtin.light')
        page.wait_for_function('window.__pending.length===1')
        transition = page.evaluate('window.__transitions.at(-1)')
        report['rawNotice'] = raw_notice
        report['transitionBeforeNewReadiness'] = transition
        report['staleLocalNoticeInSelectedLightCommit'] = transition['selected'] == 'builtin.light' and transition['notice'] == raw_notice
        if a.before:
            assert report['staleLocalNoticeInSelectedLightCommit'], 'No stale selected-style identity reproduced'
            assert transition['submitEnabled'], 'No borrowed submission eligibility reproduced'
        else:
            assert not report['staleLocalNoticeInSelectedLightCommit'], 'Old local readiness reached a new selected-style commit'
            expect(run).to_be_disabled()
            # Switching before the Light lookup returns must not let its late
            # result grant a service-dependent style eligibility or local label.
            dialog.get_by_role('combobox').select_option('builtin.formal')
            page.wait_for_function('window.__pending.length===2')
            page.evaluate('window.__pending[0].resolve({available:true,requiresTextService:false,textServiceConfigured:false})')
            expect(run).to_be_disabled()
            assert dialog.locator('[data-style-trial-service-notice]').inner_text() != raw_notice
            page.evaluate('window.__pending[1].resolve({available:true,requiresTextService:true,textServiceConfigured:false})')
            expect(dialog.get_by_role('status')).to_contain_text('This style needs a text service.')
            expect(run).to_be_disabled()
            expect(draft).to_have_value('A local draft must not imply that a different style is local.')
            report['lateOtherStyleResponseCannotGrantLocalOrSubmit'] = True
            assert 'try_style_pack_text' not in page.evaluate('window.__calls')
            report['noProcessingOrCloudRequest'] = True
        report['passed'] = True
    except Exception as error:
        report.update(passed=False, error=str(error))
    finally:
        browser.close()
        if original_component:
            original_component.unlink(missing_ok=True)
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
print(json.dumps(report, ensure_ascii=False))
raise SystemExit(0 if report['passed'] else 1)
