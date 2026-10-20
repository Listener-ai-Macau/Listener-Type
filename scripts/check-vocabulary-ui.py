"""Exercise vocabulary acknowledgement/error paths without user data or cloud calls.

--before uses a frozen prior browser build. Otherwise use an isolated Vite dev
server: only its IPC module response is wrapped to control pending/failing writes.
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
          window.__vocabMode = 'ok'; window.__vocabCalls = []; window.__vocabPending = []; window.__vocabReads = [];
          window.__unhandled = [];
          addEventListener('unhandledrejection', event => window.__unhandled.push(String(event.reason)));
          window.__vocabTestInvoke = (cmd, args, mock) => {
            if (cmd === 'list_vocab' && window.__vocabMode === 'pending-list') {
              const snapshot = mock();
              return new Promise(resolve => window.__vocabReads.push(() => resolve(snapshot)));
            }
            if (!['add_vocab','remove_vocab','set_vocab_enabled'].includes(cmd)) return mock();
            window.__vocabCalls.push(cmd);
            if (window.__vocabMode === 'fail-' + cmd) throw new Error('fixture write denied');
            if (window.__vocabMode === 'pending' && cmd === 'add_vocab') {
              return new Promise((resolve, reject) => window.__vocabPending.push({
                run: () => { try { resolve(mock()); } catch (error) { reject(error); } }, reject
              }));
            }
            return mock();
          };
        """)
        wrapped = []
        if not args.before:
            def wrap_ipc(route):
                response = route.fetch()
                script = response.text()
                old = 'if (!isTauri) {\n    return mock();\n  }'
                assert old in script, 'Expected unmodified Vite IPC mock boundary'
                script = script.replace(old, 'if (!isTauri) {\n    return window.__vocabTestInvoke(cmd, args, mock);\n  }', 1)
                wrapped.append(True)
                route.fulfill(response=response, body=script)
            page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), wrap_ipc)

        page.goto(args.url, wait_until='networkidle')
        if not args.before:
            check('Failure fixture applied at the browser mock boundary', len(wrapped) >= 1)
        banner = page.get_by_role('note')
        if args.before:
            check('Pre-fix counterexample: simulated readiness has no visible preview label', banner.count() == 0)
        else:
            expect(banner).to_contain_text('sample data')
            expect(banner).to_contain_text('does not record or insert speech')
            check('Preview labels service/device/history sample status and session scope',
                  'Changes last for this page session' in banner.inner_text())
        page.get_by_role('button', name='Vocabulary', exact=True).click()
        field = page.get_by_placeholder('Type a word, press Enter or click Add…', exact=True)
        field.wait_for()
        add = page.get_by_role('button', name='Add', exact=True).last
        sdk = page.get_by_role('button', name='SDK', exact=True)
        field.fill('SDK')
        add.click()
        expect(field).to_have_value('')
        if args.before:
            check('Pre-fix counterexample: SDK is absent after add', sdk.count() == 0)
            page.get_by_role('button', name='Refresh', exact=True).click()
            page.wait_for_timeout(100)
            check('Pre-fix counterexample: refresh still cannot find SDK', sdk.count() == 0)
        else:
            expect(sdk).to_have_count(1)
            page.get_by_role('button', name='Refresh', exact=True).click()
            expect(sdk).to_have_count(1)
            page.get_by_role('button', name='History', exact=True).click()
            page.get_by_role('button', name='Vocabulary', exact=True).click()
            page.wait_for_timeout(550)
            check('Returning during navigation cancels the fade and keeps content opaque',
                  page.locator('main > .ol-thinscroll').evaluate('(el) => Number(getComputedStyle(el).opacity) === 1'))
            expect(sdk).to_have_count(1)
            check('Acknowledged SDK survives refresh and page navigation in this session', sdk.count() == 1)

            page.evaluate("window.__vocabMode = 'fail-add_vocab'")
            field.fill('KeepDraft')
            add.click()
            expect(page.get_by_role('alert')).to_contain_text('fixture write denied')
            check('Failed add keeps draft and existing entries visible', field.input_value() == 'KeepDraft' and sdk.is_visible())
            page.evaluate("window.__vocabMode = 'ok'")
            add.click()
            expect(page.get_by_role('button', name='KeepDraft', exact=True)).to_have_count(1)
            check('Successful retry displays acknowledged entry and clears failure', page.get_by_role('alert').count() == 0)

            page.evaluate("window.__vocabMode = 'pending-list'")
            page.get_by_role('button', name='Refresh', exact=True).click()
            page.wait_for_function('window.__vocabReads.length === 1')
            page.evaluate("window.__vocabMode = 'ok'")
            field.fill('NewAck')
            add.click()
            expect(page.get_by_role('button', name='NewAck', exact=True)).to_have_count(1)
            page.evaluate('window.__vocabReads[0]()')
            page.wait_for_timeout(100)
            check('An older list response cannot overwrite an acknowledged new entry',
                  page.get_by_role('button', name='NewAck', exact=True).count() == 1)

            page.evaluate("window.__vocabMode = 'pending'")
            field.fill('LateEntry')
            add.click()
            page.wait_for_function('window.__vocabPending.length === 1')
            count = page.evaluate('window.__vocabCalls.length')
            field.fill('NextDraft')
            field.press('Enter')
            check('Pending add is single-flight', add.is_disabled() and page.evaluate('window.__vocabCalls.length') == count)
            page.evaluate('window.__vocabPending[0].run()')
            expect(page.get_by_role('button', name='LateEntry', exact=True)).to_have_count(1)
            check('Late acknowledgement preserves newer input', field.input_value() == 'NextDraft')

            page.evaluate("window.__vocabMode = 'fail-set_vocab_enabled'")
            sdk.click()
            expect(page.get_by_role('alert')).to_contain_text('fixture write denied')
            check('Failed toggle restores state without hiding the list', sdk.get_attribute('title') == 'Click to disable this entry')
            page.evaluate("window.__vocabMode = 'fail-remove_vocab'")
            sdk.locator('..').get_by_role('button', name='Remove', exact=True).click()
            expect(page.get_by_role('alert')).to_contain_text('fixture write denied')
            check('Failed delete keeps the acknowledged entry', sdk.is_visible())
            page.evaluate("window.__vocabMode = 'ok'")
            sdk.locator('..').get_by_role('button', name='Remove', exact=True).click()
            expect(sdk).to_have_count(0)
            page.get_by_role('button', name='Refresh', exact=True).click()
            expect(sdk).to_have_count(0)
            check('Successful delete survives refresh', sdk.count() == 0)
            check('No unhandled promise rejections', page.evaluate('window.__unhandled.length === 0'))
            if args.screenshot:
                args.screenshot.parent.mkdir(parents=True, exist_ok=True)
                page.wait_for_timeout(400)
                page.screenshot(path=str(args.screenshot))
    finally:
        browser.close()
report = {'at': datetime.datetime.now().astimezone().isoformat(), 'url': args.url, 'before': args.before,
          'scope': 'Isolated browser mock data; controlled IPC rejection/latency only in test response. No Type, OS clipboard, real microphone, user dictionary or cloud call.',
          'checks': checks, 'passed': all(c['passed'] for c in checks)}
args.report.parent.mkdir(parents=True, exist_ok=True)
args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps(report, ensure_ascii=False))
