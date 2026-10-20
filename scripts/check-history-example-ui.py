"""Check labelled browser examples and recovery from an empty History filter.

Run --before against frozen73, or use the candidate frontend without that flag.
All delete/clear actions affect only this isolated browser's mock history.
"""
import argparse
import datetime
import json
from pathlib import Path
from playwright.sync_api import expect, sync_playwright

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', required=True)
parser.add_argument('--report', type=Path, required=True)
parser.add_argument('--before', action='store_true')
parser.add_argument('--screenshot', type=Path)
args = parser.parse_args()
checks = []


def check(name, passed):
    checks.append({'check': name, 'passed': bool(passed)})
    assert passed, name


with sync_playwright() as p:
    browser = p.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width': 1180, 'height': 1080})
        page.set_default_timeout(6000)
        page.add_init_script("""
          localStorage.setItem('ol.locale','en');
          sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
          sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
        """)
        page.goto(args.url, wait_until='networkidle')
        page.get_by_role('button', name='History', exact=True).click()
        formal_filter = page.get_by_role('button', name='Formal', exact=True)
        formal_filter.wait_for()
        # Recent transcripts is intentionally unfiltered; inspect only the list
        # owned by the style filter, not every scrollable list on the page.
        rows = formal_filter.locator('xpath=../../..').locator('.ol-thinscroll > button')
        expect(rows.first).to_be_visible()
        page.get_by_role('button', name='Formal', exact=True).click()
        if args.before:
            expect(rows).to_have_count(0)
            check('Pre-fix: Formal has no example', True)
            expect(page.get_by_text('No history yet. Press Right Ctrl to record one.', exact=True)).to_be_visible()
            check('Pre-fix: empty filter incorrectly says there is no history', True)
        else:
            expect(rows).to_have_count(1)
            expect(page.get_by_role('note').filter(has_text='Hand-written style examples.')).to_be_visible()
            check('Examples clearly identify hand-written text, not recording or model output', True)
            expect(page.get_by_text('Hey, could you send the updated schedule by Friday? Thanks.', exact=True)).to_be_visible()
            expect(page.get_by_text('Could you please send the updated schedule by Friday? Thank you.', exact=True).last).to_be_visible()
            expect(page.get_by_text('Illustration · no recording or insertion', exact=True)).to_be_visible()
            check('Formal offers a concrete before/after with no insertion-success claim', True)
            if args.screenshot:
                args.screenshot.parent.mkdir(parents=True, exist_ok=True)
                page.screenshot(path=str(args.screenshot), full_page=True)
            for name in ('Raw','Light polish','Structured','Formal'):
                button = page.get_by_role('button', name=name, exact=True)
                button.click()
                expect(rows).to_have_count(1)
            check('Every supported style has an illustrative entry', True)
            page.get_by_role('button', name='Delete', exact=True).click()
            expect(rows).to_have_count(0)
            expect(page.get_by_text('No entries match this style. Your other history is still available.', exact=True)).to_be_visible()
            page.get_by_role('button', name='Show all history', exact=True).click()
            expect(rows).to_have_count(3)
            check('Empty-filter recovery keeps the other entries visible', True)
            page.on('dialog', lambda dialog: dialog.accept())
            page.get_by_role('button', name='Clear', exact=True).click()
            expect(rows).to_have_count(0)
            expect(page.get_by_text('No history yet. Press Right Ctrl to record one.', exact=True)).to_be_visible()
            check('An actually empty history retains its own first-recording message', True)
    finally:
        browser.close()

report = {'at': datetime.datetime.now().astimezone().isoformat(), 'url': args.url,
          'before': args.before, 'scope': __doc__, 'checks': checks,
          'passed': all(c['passed'] for c in checks)}
args.report.parent.mkdir(parents=True, exist_ok=True)
args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps(report, ensure_ascii=False))
