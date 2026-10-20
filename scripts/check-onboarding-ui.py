"""Replay microphone onboarding and direct local setup without devices or cloud."""
import argparse
import datetime
import json
from pathlib import Path
from playwright.sync_api import expect, sync_playwright

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
p.add_argument('--before', action='store_true')
p.add_argument('--screenshot', type=Path)
a = p.parse_args()
checks = []


def check(name, ok):
    checks.append({'check': name, 'passed': bool(ok)})
    assert ok, name


with sync_playwright() as pw:
    b = pw.chromium.launch(headless=True, channel='msedge')
    try:
        page = b.new_page(viewport={'width': 1180, 'height': 1000})
        page.set_default_timeout(6000)
        page.add_init_script("""
          localStorage.setItem('ol.locale','en');
          sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
        """)
        page.goto(a.url, wait_until='networkidle')
        page.get_by_role('button', name='Microphone and input source', exact=True).click()
        page.get_by_role('button', name='Microphone', exact=True).click()
        pairing = page.get_by_text('Pair your Listener device', exact=True)
        if a.before:
            expect(pairing).to_be_visible()
            check('Pre-fix: choosing microphone opens an unrelated hardware guide', True)
        else:
            mic = page.get_by_role('button', name='Microphone', exact=True)
            expect(mic).to_have_attribute('aria-pressed', 'true')
            page.wait_for_timeout(300)
            check('Choosing microphone leaves recording settings unobstructed', pairing.count() == 0)
            page.get_by_role('button', name='Listener BLE', exact=True).click()
            page.get_by_role('button', name='Set up Listener (optional)', exact=True).click()
            expect(pairing).to_be_visible()
            check('Hardware guide remains available by explicit choice', True)
            page.get_by_role('button', name='Use microphone', exact=True).click()
            expect(pairing).to_have_count(0)
            expect(mic).to_have_attribute('aria-pressed', 'true')
            check('Guide microphone choice actually selects microphone before closing', True)
            page.get_by_role('button', name='Providers', exact=True).click()
            page.get_by_role('button', name='Set up local recognition', exact=True).click()
            expect(page.locator('.ol-settings-nav .ol-nav-btn-active')).to_have_text('Local recognition')
            expect(page.get_by_text('Windows Foundry Local Whisper', exact=True)).to_be_visible()
            check('Local setup starts directly at its model preparation, not insertion internals', page.get_by_text('Streaming insertion', exact=True).count() == 0)
            if a.screenshot:
                a.screenshot.parent.mkdir(parents=True, exist_ok=True)
                page.screenshot(path=str(a.screenshot), full_page=True)
    finally:
        b.close()
report = {'at': datetime.datetime.now().astimezone().isoformat(), 'url': a.url,
          'before': a.before, 'scope': 'Isolated headless browser mocks; no Type, microphone, cloud, model download, clipboard or hardware changes.',
          'checks': checks, 'passed': all(x['passed'] for x in checks)}
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps(report, ensure_ascii=False))

