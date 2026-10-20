"""Exercise History copy ownership in an isolated browser, without the OS clipboard.

Serve a frontend build with its browser mock backend, then pass --url.
Requires Playwright and the installed Microsoft Edge browser.
"""
import argparse
import datetime
import json
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', required=True)
parser.add_argument('--report', type=Path)
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
          window.testCopies = [];
          window.testCopyResolvers = [];
          window.testCopyDelay = false;
          window.testCopyFail = false;
          Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {
            writeText: text => {
              window.testCopies.push(text);
              if (window.testCopyFail) return Promise.reject(new Error('clipboard denied'));
              return window.testCopyDelay
                ? new Promise(resolve => window.testCopyResolvers.push(resolve))
                : Promise.resolve();
            }
          }});
        """)

        def open_history():
            page.goto(args.url, wait_until='networkidle')
            page.get_by_role('button', name='History', exact=True).click()
            rows = page.locator('.ol-thinscroll > button')
            rows.nth(1).wait_for()
            return rows

        def copy():
            page.get_by_role('button', name='Copy', exact=True).click()

        def copied():
            return page.get_by_role('button', name='Copied', exact=True)

        rows = open_history()
        copy()
        copied().wait_for()
        rows.nth(1).click()
        check('Selecting B after copying A does not claim B was copied', copied().count() == 0)
        rows.nth(0).click()
        check('Success belongs to the record and text actually copied', copied().count() == 1)

        rows = open_history()
        page.evaluate('window.testCopyDelay = true')
        copy()
        page.wait_for_function('window.testCopyResolvers.length === 1')
        rows.nth(1).click()
        check('Another copy cannot overtake a pending clipboard write',
              page.get_by_role('button', name='Copy', exact=True).is_disabled()
              and page.evaluate('window.testCopies.length === 1'))
        page.evaluate('window.testCopyResolvers[0]()')
        expect(page.get_by_role('button', name='Copy', exact=True)).to_be_enabled()
        check('Late A completion does not label B copied', copied().count() == 0)
        page.evaluate('window.testCopyDelay = false')
        copy()
        copied().wait_for()
        check('B can be copied after A completes, with its own payload',
              page.evaluate('window.testCopies.length === 2 && window.testCopies[0] !== window.testCopies[1]'))

        rows = open_history()
        copy()
        copied().wait_for()
        page.wait_for_timeout(850)
        rows.nth(1).click()
        copy()
        copied().wait_for()
        page.wait_for_timeout(850)
        check('A previous success timer cannot clear the newer copy feedback', copied().count() == 1)
        expect(copied()).to_have_count(0)

        rows = open_history()
        page.evaluate('window.testCopyFail = true')
        copy()
        page.get_by_text('clipboard denied', exact=False).wait_for()
        check('A rejected clipboard write reports failure and never success',
              copied().count() == 0 and page.get_by_role('button', name='Copy', exact=True).is_enabled())
    finally:
        browser.close()

report = {
    'at': datetime.datetime.now().astimezone().isoformat(),
    'url': args.url,
    'scope': 'Isolated frontend with browser mock history and clipboard stub; no Type, OS clipboard or real history mutation.',
    'checks': checks,
    'passed': all(c['passed'] for c in checks),
}
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2), encoding='utf-8')
print(json.dumps(report))
