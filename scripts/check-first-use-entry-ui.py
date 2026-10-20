"""Check setup navigation and start failures in an isolated browser/mock frontend.

Pass --before for the frozen pre-fix build. The start callback is replaced in the
browser response only; no microphone, user preferences or cloud service is used.
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
args = parser.parse_args()
checks = []


def check(name, condition):
    checks.append({'check': name, 'passed': bool(condition)})
    assert condition, name


with sync_playwright() as p:
    browser = p.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width': 1180, 'height': 1000})
        page.set_default_timeout(6000)
        page.add_init_script("""
          localStorage.setItem('ol.locale', 'en');
          if (!localStorage.getItem('ol.dev.credentialsState')) {
            localStorage.setItem('ol.dev.credentialsState', 'asr-only');
          }
          sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession', '1');
          sessionStorage.setItem('ol.blePairingPromptDeferredThisSession', '1');
          window.__startCalls = 0;
          window.__startResolvers = [];
        """)
        replacements = []

        def fixture_start(route):
            response = route.fetch()
            script = response.text()
            script, count = re.subn(
                r'("start_dictation",void 0,\(\)=>)\{\}',
                r'\1{window.__startCalls++;return new Promise((resolve,reject)=>window.__startResolvers.push({resolve,reject}));}',
                script,
            )
            replacements.extend([route.request.url] * count)
            route.fulfill(response=response, body=script)

        page.route('**/assets/*.js', fixture_start)
        page.goto(args.url, wait_until='networkidle')
        page.get_by_title('Settings', exact=True).click()
        active = page.locator('.ol-settings-nav .ol-nav-btn-active')
        expect(active).to_have_text('Device')
        check('Existing Listener input still opens Device', active.inner_text() == 'Device')
        page.get_by_role('button', name='Recording', exact=True).click()
        page.get_by_role('button', name='Microphone', exact=True).click()
        page.wait_for_timeout(100)
        page.get_by_title('Close', exact=True).click()
        page.get_by_title('Settings', exact=True).click()
        expected = 'Device' if args.before else 'Recording'
        expect(active).to_have_text(expected)
        check('Pre-fix counterexample: microphone user still sees Device' if args.before else
              'Microphone user reopens directly in Recording', active.inner_text() == expected)
        if not args.before:
            page.get_by_role('button', name='Listener BLE', exact=True).click()
            page.wait_for_timeout(100)
            check('Changing input while editing does not redirect the section', active.inner_text() == 'Recording')
            page.get_by_title('Close', exact=True).click()
            page.get_by_title('Settings', exact=True).click()
            expect(active).to_have_text('Device')
            check('Reopening after choosing Listener uses Device', active.inner_text() == 'Device')
            page.get_by_title('Close', exact=True).click()
            page.get_by_role('button', name='Microphone and input source', exact=True).click()
            expect(active).to_have_text('Recording')
            check('Explicit Recording entry overrides Listener default', active.inner_text() == 'Recording')
        page.get_by_title('Close', exact=True).click()

        start_label = 'Try microphone recording' if args.before else 'Start dictation'
        page.get_by_role('button', name=start_label, exact=True).click()
        page.wait_for_function('window.__startResolvers.length === 1')
        if not args.before:
            check('Pending start is disabled and issued exactly once',
                  page.get_by_role('button', name='Starting…', exact=True).is_disabled()
                  and page.evaluate('window.__startCalls === 1'))
        page.evaluate('window.__startResolvers[0].reject(new Error("fixture input unavailable"))')
        if args.before:
            page.wait_for_timeout(100)
            check('Pre-fix counterexample: start error is swallowed',
                  page.get_by_role('alert').count() == 0)
        else:
            expect(page.get_by_role('alert')).to_contain_text('Could not start dictation: fixture input unavailable')
            check('Start failure is visible with actual reason', page.get_by_role('alert').is_visible())
            page.get_by_role('button', name='Start dictation', exact=True).click()
            page.wait_for_function('window.__startResolvers.length === 2')
            page.evaluate('window.__startResolvers[1].resolve()')
            expect(page.get_by_role('button', name='Start dictation', exact=True)).to_be_enabled()
            check('Successful retry clears the previous failure', page.get_by_role('alert').count() == 0)
        check('Start fixture was applied to the real frontend module', len(replacements) >= 1)

        page.evaluate("localStorage.setItem('ol.dev.credentialsState', 'missing')")
        page.reload(wait_until='networkidle')
        expected_count = 1 if args.before else 0
        check('Pre-fix counterexample: start offered before recognition setup' if args.before else
              'No start action offered before recognition setup',
              page.get_by_role('button', name=start_label, exact=True).count() == expected_count)
    finally:
        browser.close()

report = {
    'at': datetime.datetime.now().astimezone().isoformat(),
    'url': args.url, 'before': args.before,
    'scope': 'Isolated browser/mock preferences, start promise fixture in browser response only; no real microphone, cloud call, Type or OS input.',
    'checks': checks, 'passed': all(c['passed'] for c in checks),
}
args.report.parent.mkdir(parents=True, exist_ok=True)
args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps(report, ensure_ascii=False))
