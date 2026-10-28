"""Actual React optional-shortcut setup/clear with controlled IPC only.

Private headless browser and mock preferences, no OS shortcuts, owner keys,
clipboard, microphone, cloud or hardware. Native startup needs separate proof.
"""
import argparse
import datetime
import json
import re
from pathlib import Path
from playwright.sync_api import expect, sync_playwright

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
a = p.parse_args()
a.report.parent.mkdir(parents=True, exist_ok=True)
report = {'at': datetime.datetime.now().astimezone().isoformat(), 'scope': __doc__, 'checks': [], 'passed': False}
fields = ['dictationHotkey', 'translationHotkey', 'qaHotkey', 'switchStyleHotkey', 'openAppHotkey']

with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge', args=['--disable-gpu'])
    page = browser.new_page(viewport={'width': 1050, 'height': 800})
    try:
        page.add_init_script("""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.__shortcutCalls=[];window.__shortcutFail=false;window.__shortcutDelay=false;
window.__shortcutFixture=(cmd,args,mock)=>{
 window.__shortcutCalls.push({cmd,args});
 if(cmd==='get_credentials')return {...mock(),asrConfigured:true,llmConfigured:true};
 if(cmd.startsWith('set_')&&cmd.endsWith('_hotkey')){
  if(window.__shortcutFail)return Promise.reject('fixture write failed');
  if(window.__shortcutDelay)return new Promise(resolve=>window.__finishShortcut=()=>resolve(mock()));
 }
 return mock();
};
""")

        def fixture(route):
            response = route.fetch()
            source = response.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert source.count(old) == 1 and 'let mockSettings =' in source
            source = source.replace(old, 'if (!isTauri) {\n    return window.__shortcutFixture(cmd,args,mock);\n  }', 1)
            source += '\nwindow.__readShortcutPrefs=()=>structuredClone(mockSettings);window.__patchShortcutPrefs=p=>Object.assign(mockSettings,p);\n'
            route.fulfill(response=response, body=source)

        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), fixture)
        page.goto(a.url, wait_until='networkidle')
        prefs = page.evaluate('window.__readShortcutPrefs()')
        assert prefs['qaHotkey'] is None
        assert all(prefs[field] == {'primary': '', 'modifiers': []} for field in fields if field != 'qaHotkey')
        page.get_by_title('Settings', exact=True).click()
        page.get_by_role('button', name='Shortcuts', exact=True).click()
        recorders = page.locator('[data-shortcut-recorder]')
        expect(recorders).to_have_count(5)
        expect(page.locator('[data-shortcut-clear]')).to_have_count(0)
        for recorder in recorders.all():
            expect(recorder.locator('span').first).to_have_text('Not set')
        assert not any(x['cmd'].startswith('set_') and x['cmd'].endswith('_hotkey') for x in page.evaluate('window.__shortcutCalls'))
        report['checks'].append('Fresh actual mock preferences and all five settings recorders are unassigned; opening the page assigns no binding and no QA Enable preset exists')

        for index, field in enumerate(fields):
            row = recorders.nth(index)
            row.locator('[data-shortcut-record]').click()
            page.keyboard.press('Control+Alt+K')
            expect(row.locator('[data-shortcut-clear]')).to_be_visible()
            saved = page.evaluate('window.__readShortcutPrefs()')
            assert saved[field] == {'primary': 'K', 'modifiers': ['ctrl', 'alt']}
            assert all(saved[other] is None if other == 'qaHotkey' else saved[other] == {'primary': '', 'modifiers': []} for other in fields if other != field)
            row.locator('[data-shortcut-clear]').click()
            expect(row.locator('[data-shortcut-clear]')).to_have_count(0)
            saved = page.evaluate('window.__readShortcutPrefs()')
            assert saved[field] == (None if field == 'qaHotkey' else {'primary': '', 'modifiers': []})
        report['checks'].append('Each shortcut is assigned only by explicit recording; clearing each restores its empty/null state without assigning or changing the other four')

        row = recorders.nth(1)
        row.locator('[data-shortcut-record]').click()
        page.keyboard.press('Control+Alt+K')
        expect(row.locator('[data-shortcut-clear]')).to_be_visible()
        page.evaluate('window.__shortcutFail=true')
        row.locator('[data-shortcut-clear]').click()
        expect(row).to_contain_text('Could not confirm')
        assert page.evaluate('window.__readShortcutPrefs().translationHotkey.primary') == 'K'
        page.evaluate('window.__shortcutFail=false;window.__shortcutDelay=true')
        row.locator('[data-shortcut-clear]').click()
        expect(row.locator('[data-shortcut-clear]')).to_be_disabled()
        expect(row.locator('[data-shortcut-record]')).to_be_disabled()
        page.evaluate('window.__finishShortcut();window.__shortcutDelay=false')
        expect(row.locator('[data-shortcut-clear]')).to_have_count(0)
        report['checks'].append('Failed clear never reports unassigned or changes stored binding; pending clear disables capture/clear until acknowledged, then returns to unassigned')

        before = len([x for x in page.evaluate('window.__shortcutCalls') if x['cmd'].startswith('set_') and x['cmd'].endswith('_hotkey')])
        recorders.nth(0).locator('[data-shortcut-record]').click()
        page.keyboard.press('Escape')
        after = len([x for x in page.evaluate('window.__shortcutCalls') if x['cmd'].startswith('set_') and x['cmd'].endswith('_hotkey')])
        assert before == after
        page.screenshot(path=str(a.report.with_suffix('.png')))
        report['checks'].append('Cancelling shortcut capture does not save or invent a binding; contextual Escape/confirmation rows remain distinct from optional global assignments')
        report['passed'] = True
    except Exception as error:
        report['failure'] = str(error)
        page.screenshot(path=str(a.report.with_name(a.report.stem + '-failure.png')))
        raise
    finally:
        browser.close()
        a.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
print(json.dumps({'checks': len(report['checks']), 'passed': report['passed']}))
