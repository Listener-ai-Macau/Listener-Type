"""Replay customer setup confusion and delayed-settings rendering in isolation.

Use --before with frozen72; otherwise use the candidate Vite server. All writes
are browser mocks. No native Type, microphone, system clipboard or cloud calls.
"""
import argparse
import datetime
import json
import re
from pathlib import Path
from playwright.sync_api import expect, sync_playwright

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', required=True)
parser.add_argument('--report', type=Path, required=True)
parser.add_argument('--before', action='store_true')
parser.add_argument('--screenshot', type=Path)
args = parser.parse_args()
checks = []


def check(name, value):
    checks.append({'check': name, 'passed': bool(value)})
    assert value, name


with sync_playwright() as p:
    browser = p.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width': 1180, 'height': 1000})
        page.set_default_timeout(6000)
        page.add_init_script("""
          localStorage.setItem('ol.locale','en');
          sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
          sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
          window.__pendingSettings=[]; window.__commands=[];
          window.__settingsDelay=false;
          window.__settingsFixture=(cmd,args,mock)=>{
            window.__commands.push(cmd);
            if(cmd==='get_settings' && window.__settingsDelay)
              return new Promise(resolve=>window.__pendingSettings.push(()=>resolve(mock())));
            return mock();
          };
        """)
        fixture_count = []

        def fixture(route):
            response = route.fetch()
            script = response.text()
            if args.before:
                script, n = re.subn(
                    r'("get_settings",void 0,\(\)=>)\(\{\.\.\.(\w+)\}\)',
                    r'\1window.__settingsFixture("get_settings",null,()=>({...\2}))', script)
            else:
                old = 'if (!isTauri) {\n    return mock();\n  }'
                n = script.count(old)
                script = script.replace(old, 'if (!isTauri) {\n    return window.__settingsFixture(cmd,args,mock);\n  }', 1)
            fixture_count.extend([True] * n)
            route.fulfill(response=response, body=script)

        page.route('**/assets/*.js' if args.before else re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), fixture)
        page.goto(args.url, wait_until='networkidle')
        check('IPC fixture applied only in the isolated browser', bool(fixture_count))
        page.get_by_title('Settings', exact=True).click()
        page.get_by_role('button', name='Providers', exact=True).click()
        local = page.get_by_role('button', name='Set up local recognition', exact=True)
        if args.before:
            check('Pre-fix: Providers has no path to local setup', local.count() == 0)
        else:
            expect(local).to_be_visible()
            local.click()
            expect(page.locator('.ol-settings-nav .ol-nav-btn-active')).to_have_text('Local recognition')
            check('Local setup reaches the existing Local recognition page', True)
            check('Navigation neither activates nor downloads a model', not any(
                'prepare' in c or 'download' in c or c == 'set_active_asr_provider'
                for c in page.evaluate('window.__commands')))
        page.get_by_title('Close', exact=True).click()
        page.locator('aside nav').get_by_role('button', name='Tools', exact=True).click()
        page.locator('[data-tool="translation"]').click()
        target = page.get_by_role('combobox', name='Translation target language', exact=True)
        expect(target).to_be_visible()
        desc = page.get_by_text('Optional: select languages you regularly use to guide spelling and writing style. This does not choose the translation output language.', exact=True)
        if args.before:
            check('Pre-fix: working-language explanation is not rendered', desc.count() == 0)
        else:
            expect(desc).to_be_visible()
            group = page.get_by_role('group', name='Working languages', exact=True)
            check('Translation output choice appears before working languages', target.bounding_box()['y'] < group.bounding_box()['y'])
            target.click()
            page.get_by_role('option', name='English', exact=True).click()
            expect(target).to_have_text('English')
            english = group.get_by_role('button', name='English', exact=True)
            before = english.get_attribute('aria-pressed')
            english.click()
            expect(english).to_have_attribute('aria-pressed', 'false' if before == 'true' else 'true')
            expect(target).to_have_text('English')
            check('Working-language toggle exposes state and leaves output unchanged', True)
            page.get_by_title('Settings', exact=True).click()
            page.get_by_role('button', name='Recording', exact=True).click()
            page.get_by_text('History & context', exact=True).click()
            raw = page.get_by_role('switch', name='Keep raw recording (debug)', exact=True)
            expect(raw).to_be_visible()
            before = raw.get_attribute('aria-checked')
            check('Raw recording exposes its actual mock on/off state', before in ('true', 'false'))
            raw.click()
            expect(raw).to_have_attribute('aria-checked', 'false' if before == 'true' else 'true')
            raw.click()
            expect(raw).to_have_attribute('aria-checked', before)
            check('Switch remains operable and restores its prior mock value', True)
            page.get_by_title('Close', exact=True).click()
            if args.screenshot:
                args.screenshot.parent.mkdir(parents=True, exist_ok=True)
                page.screenshot(path=str(args.screenshot), full_page=True)

        # Hold initial preferences until Translation has rendered its loading UI.
        page.add_init_script('window.__settingsDelay=true;')
        page.reload(wait_until='networkidle')
        page.locator('aside nav').get_by_role('button', name='Tools', exact=True).click()
        page.locator('[data-tool="translation"]').click()
        page.wait_for_function('window.__pendingSettings.length>0')
        page.wait_for_timeout(350)
        page.evaluate('window.__settingsDelay=false;window.__pendingSettings.splice(0).forEach(resolve=>resolve());')
        if args.before:
            expect(page.locator('body')).to_contain_text(re.compile(r'#310|Rendered more hooks'))
            check('Pre-fix: delayed preferences crash Translation through conditional hooks', True)
        else:
            expect(page.get_by_role('combobox', name='Translation target language', exact=True)).to_be_visible()
            check('Translation recovers from initial loading without conditional hooks', True)
    finally:
        browser.close()

report = {'at': datetime.datetime.now().astimezone().isoformat(), 'url': args.url,
          'before': args.before, 'scope': __doc__, 'checks': checks,
          'passed': all(c['passed'] for c in checks)}
args.report.parent.mkdir(parents=True, exist_ok=True)
args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps(report, ensure_ascii=False))
