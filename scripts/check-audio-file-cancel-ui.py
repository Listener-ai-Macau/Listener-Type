"""File-input cancellation must not close the recording workspace.

The bubbling file-input event was observed in the shipped Windows WebView.
This focused React check uses the real component, with no recognition request.
Actual Windows chooser integration is qualified separately.
"""
import argparse
import json
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

parser = argparse.ArgumentParser()
parser.add_argument('--url', required=True)
parser.add_argument('--report', required=True, type=Path)
args = parser.parse_args()

with sync_playwright() as playwright:
    browser = playwright.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width': 1280, 'height': 1000})
        page.set_default_timeout(6500)
        page.add_init_script("""
          localStorage.setItem('ol.locale','en');
          sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
        """)
        page.goto(args.url, wait_until='networkidle')
        page.get_by_role('button', name='Tools', exact=True).first.click()
        page.locator('details > summary').get_by_text('More tools', exact=True).click()
        page.get_by_role('button', name='Transcribe recorded audio', exact=True).click()
        dialog = page.get_by_role('dialog', name='Transcribe recorded audio', exact=True)
        file_input = dialog.get_by_label('Choose WAV file', exact=True)
        file_input.dispatch_event('cancel', {'bubbles': True})
        expect(dialog).to_be_visible()
        dialog.get_by_role('button', name='Use included sample', exact=True).click()
        sample = dialog.get_by_text('Included synthetic English sample · 11.5 seconds', exact=True)
        expect(sample).to_be_visible()
        file_input.dispatch_event('cancel', {'bubbles': True})
        expect(dialog).to_be_visible()
        expect(sample).to_be_visible()
        expect(file_input).to_be_enabled()
        # Genuine dialog Escape still closes it; filtering the descendant event
        # must not disable the normal close behavior.
        dialog.press('Escape')
        expect(dialog).not_to_be_visible()
        page.get_by_role('button', name='Tools', exact=True).first.click()
        details = page.locator('details').filter(has=page.get_by_text('More tools', exact=True))
        if details.get_attribute('open') is None:
            details.locator('summary').click()
        page.get_by_role('button', name='Transcribe recorded audio', exact=True).click()
        expect(dialog).to_be_visible()
        expect(dialog.get_by_text('No audio selected.', exact=True)).to_be_visible()
        result = {
            'passed': True,
            'scope': 'Actual React component with browser file-input cancel events; no native chooser, ASR, device or external input qualification.',
            'checks': [
                'Cancelling a file choice keeps the empty recording workspace open',
                'Cancelling another choice preserves the selected local sample and usable file input',
                'Real dialog Escape still closes the workspace and reopening starts a fresh selection',
            ],
        }
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf8')
        print(json.dumps(result, ensure_ascii=False))
    except Exception:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(args.report.with_suffix('.failure.png')))
        args.report.with_suffix('.failure.txt').write_text(page.locator('body').inner_text(), encoding='utf8')
        raise
    finally:
        browser.close()
