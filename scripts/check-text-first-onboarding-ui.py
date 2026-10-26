"""Actual React launch/history/configuration UI with explicit keyless IPC fixtures.
No native microphone, cloud, device, clipboard or owner desktop operation.
"""
import argparse
import datetime
import json
import re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
p.add_argument('--expected-build')
p.add_argument('--capture-before', action='store_true')
a = p.parse_args()
checks, observations = [], {}
fixture = """
localStorage.setItem('ol.locale','en');
window.__textFirstCalls=[];
window.__textFirstFixture=(cmd,args,mock)=>{
  window.__textFirstCalls.push({cmd,args});
  if(cmd==='get_credentials') return {activeAsrProvider:'volcengine',activeLlmProvider:'ark',asrConfigured:false,llmConfigured:false,volcengineConfigured:false,arkConfigured:false};
  if(cmd==='list_history') return [];
  if(cmd==='list_saved_text_drafts') return [{id:'retained-text-fixture',createdAt:'2020-01-01T00:00:00Z',name:'Earlier draft',original:'Earlier source',text:'An earlier saved translation'}];
  return mock();
};
"""
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge')
    try:
        page = browser.new_page(viewport={'width': 1180, 'height': 900})
        page.add_init_script(fixture)
        def route_ipc(route):
            response = route.fetch()
            text = response.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert old in text
            route.fulfill(response=response, body=text.replace(old, 'if (!isTauri) {\n    return window.__textFirstFixture(cmd,args,mock);\n  }', 1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), route_ipc)
        def observe_launch():
            page.wait_for_function("window.__textFirstCalls.some(x=>x.cmd==='get_credentials')")
            page.wait_for_timeout(250)
            popup = page.get_by_text('Set up speech providers', exact=True)
            shown = popup.is_visible()
            if shown:
                page.get_by_role('button', name='Later', exact=True).click()
            expect(page.get_by_role('button', name='Try typed text', exact=True)).to_be_visible()
            return shown
        page.goto(a.url, wait_until='networkidle')
        if a.expected_build:
            expect(page.locator('[data-app-build]')).to_have_attribute('data-app-build', a.expected_build)
        observations['firstLaunchProviderModal'] = observe_launch()
        checks.append({'name': 'A visible keyless launch leaves local text use accessible without dismissing an ASR modal', 'passed': not observations['firstLaunchProviderModal']})
        page.get_by_role('button', name='Try typed text', exact=True).click()
        page.locator('.ol-style-trial-input').fill('This local draft needs no speech service.')
        expect(page.locator('.ol-style-trial-input')).to_have_value('This local draft needs no speech service.')
        page.locator('dialog[open]').get_by_role('button', name='Close', exact=True).first.click()
        page.locator('nav').get_by_role('button', name='History', exact=True).click()
        empty = page.locator('[data-history-empty]')
        expect(empty).to_be_visible()
        observations['historyHeading'] = page.get_by_role('heading', name=re.compile('history', re.I)).first.inner_text()
        observations['emptyTitle'] = empty.locator('h2').inner_text()
        observations['emptyBody'] = empty.locator(':scope > p').inner_text()
        explicit_scope = 'dictation' in observations['emptyBody'].lower() and 'Q&A' in observations['emptyBody']
        checks.append({'name': 'Empty History explicitly describes dictation and Q&A, not all saved content', 'passed': explicit_scope})
        empty.get_by_role('button', name='Saved drafts', exact=True).click()
        saved = page.get_by_role('dialog', name='Saved drafts', exact=True)
        expect(saved.get_by_role('textbox', name='Saved text', exact=True)).to_have_value('An earlier saved translation')
        expect(saved.get_by_role('combobox', name='Choose a saved draft', exact=True)).to_have_value('retained-text-fixture')
        saved.get_by_role('button', name='Close', exact=True).click()
        checks.append({'name': 'Existing saved translation remains directly reachable from empty History without altering either store', 'passed': True})
        page.locator('nav').get_by_role('button', name='Overview', exact=True).click()
        page.get_by_role('button', name='Set up speech recognition first', exact=True).click()
        expect(page.locator('input[type=password]').first).to_be_visible()
        checks.append({'name': 'Explicit missing-speech setup still opens the actual provider settings', 'passed': True})
        blocked = page.evaluate("window.__textFirstCalls.filter(x=>/^(start_dictation|stop_dictation|set_credential|set_active_|save_text_draft|delete_saved_text_draft|copy_|insert_|try_|prepare_)/.test(x.cmd)).map(x=>x.cmd)")
        assert not blocked, blocked
        checks.append({'name': 'Reading drafts, editing local text and opening speech settings makes no credential/model/recording/cloud/content writes', 'passed': True})
        page.evaluate('sessionStorage.clear()')
        page.reload(wait_until='networkidle')
        observations['freshSessionProviderModal'] = observe_launch()
        checks.append({'name': 'A subsequent fresh session also opens without a deferred-until-next-launch modal', 'passed': not observations['freshSessionProviderModal']})
    finally:
        browser.close()
result = {'at': datetime.datetime.now().astimezone().isoformat(), 'scope': __doc__, 'passed': all(c['passed'] for c in checks), 'checks': checks, 'observations': observations}
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps(result, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
print(json.dumps(result, ensure_ascii=False))
if not result['passed'] and not a.capture_before:
    raise SystemExit(1)
