"""Exercise the workspace's actual viewport, explicit submission and retained edits.

Uses a private headless React page with controlled IPC responses. No download,
model, cloud request, native clipboard, user desktop or hardware is exercised.
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
parser.add_argument('--measure-only', action='store_true')
args = parser.parse_args()
report = {'at': datetime.datetime.now().astimezone().isoformat(),
          'scope': __doc__, 'viewports': [], 'checks': [], 'passed': False}
args.report.parent.mkdir(parents=True, exist_ok=True)

with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge', args=['--disable-gpu'])
    try:
        page = browser.new_page(viewport={'width': 900, 'height': 700})
        page.add_init_script("""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.__workspaceCalls=[]; window.__workspacePending=[];
window.__workspaceFixture=(cmd,args,mock)=>{
  window.__workspaceCalls.push({cmd,args});
  if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:true};
  if(cmd==='get_local_translation_status')return {supported:true,runtimeReady:true,runtimeDownloadBytes:0,pairs:[{source:'en',target:'zh',ready:true,downloadBytes:70743021},{source:'zh',target:'en',ready:true,downloadBytes:74481402}],preparing:false,receivedBytes:0,expectedBytes:0};
  if(cmd==='try_local_translation_text'||cmd==='try_translation_text')return new Promise(resolve=>window.__workspacePending.push(()=>resolve({packId:cmd==='try_local_translation_text'?'local_translation:'+args.sourceLanguage+':'+args.targetLanguage:'translation:'+args.targetLanguage,packName:'Fixture translation',outputText:cmd==='try_local_translation_text'?'本机初稿，保留完整原文。':'Cloud result from the original.',usedTextService:cmd==='try_translation_text',elapsedMs:400})));
  return mock();
};
""")

        def ipc_fixture(route):
            response = route.fetch()
            source = response.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert source.count(old) == 1
            route.fulfill(response=response, body=source.replace(old, 'if (!isTauri) {\n    return window.__workspaceFixture(cmd,args,mock);\n  }', 1))

        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), ipc_fixture)
        page.goto(args.url, wait_until='networkidle')
        page.get_by_role('button', name='Chinese ↔ English on this PC', exact=True).click()
        dialog = page.locator('dialog.ol-style-trial[open]')
        original = 'Please keep the original deadline. Do not send this email yet.'
        entry = dialog.locator('.ol-style-trial-input')
        entry.fill(original)
        dialog.get_by_role('combobox', name='Translate output to', exact=True).select_option('简体中文')
        dialog.get_by_role('combobox', name='Original language', exact=True).select_option('en')
        run = dialog.get_by_role('button', name='Translate', exact=True)
        expect(run).to_be_enabled()

        def geometry(button):
            return button.evaluate("""e=>{
 const r=e.getBoundingClientRect(),d=e.closest('dialog').getBoundingClientRect();
 const hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);
 return {button:{x:r.x,y:r.y,width:r.width,height:r.height},dialog:{x:d.x,y:d.y,width:d.width,height:d.height},fullyVisible:r.y>=Math.max(0,d.y)&&r.bottom<=Math.min(innerHeight,d.bottom)&&r.x>=0&&r.right<=innerWidth,centerHitsButton:!!hit&&e.contains(hit)};
}""")

        for width, height, scale in [(900, 700, 1), (1280, 720, 1), (600, 700, 1), (360, 640, 1), (900, 700, 1.25)]:
            page.set_viewport_size({'width': width, 'height': height})
            page.evaluate("s=>{document.documentElement.style.zoom=String(s);document.documentElement.style.setProperty('--ol-font-scale',String(s));}", scale)
            for position in ['top', 'bottom']:
                dialog.evaluate("(d,p)=>{const s=d.querySelector('[data-workspace-scroll]')||d;s.scrollTop=p==='top'?0:s.scrollHeight;}", position)
                page.wait_for_timeout(50)
                sample = {'width': width, 'height': height, 'scale': scale, 'scroll': position,
                          'submit': geometry(run), 'close': geometry(dialog.get_by_role('button', name='Close', exact=True))}
                report['viewports'].append(sample)
            if (width, height, scale) in [(900, 700, 1), (360, 640, 1)]:
                dialog.evaluate("d=>{const s=d.querySelector('[data-workspace-scroll]')||d;s.scrollTop=0}")
                page.screenshot(path=str(args.report.with_name(args.report.stem + f'-{width}x{height}.png')))
        visibility = all(s[k]['fullyVisible'] and s[k]['centerHitsButton'] for s in report['viewports'] for k in ['submit', 'close'])
        report['allMeasuredActionsVisibleAndHitTestable'] = visibility
        if not args.measure_only:
            assert visibility, 'Submit or Close is clipped or covered in the actual viewport'
            report['checks'].append('Expanded settings and all measured viewport/zoom/scroll states retain hit-testable Submit and Close')
            page.set_viewport_size({'width': 900, 'height': 700})
            page.evaluate("document.documentElement.style.zoom='1';document.documentElement.style.setProperty('--ol-font-scale','1')")
            run.click()
            page.wait_for_function('window.__workspacePending.length===1')
            cancel = dialog.get_by_role('button', name='Cancel', exact=True)
            for width, height in [(900, 700), (360, 640)]:
                page.set_viewport_size({'width': width, 'height': height})
                assert geometry(cancel)['fullyVisible'] and geometry(cancel)['centerHitsButton']
            page.set_viewport_size({'width': 900, 'height': 700})
            page.evaluate('window.__workspacePending.shift()()')
            output = dialog.get_by_role('textbox', name='Result text', exact=True)
            expect(output).to_have_value('本机初稿，保留完整原文。')
            edited = '人工修订稿。完整保留星期四与否定。'
            output.fill(edited)
            dialog.get_by_role('button', name='Switch to my cloud service…', exact=True).click()
            expect(dialog.locator('[data-cloud-retranslation-ready]')).to_be_in_viewport(ratio=1)
            expect(dialog.locator('[data-text-previous-step]')).to_contain_text(edited)
            expect(entry).to_have_value(original)
            assert page.evaluate("window.__workspaceCalls.filter(x=>x.cmd==='try_translation_text').length") == 0
            assert geometry(run)['centerHitsButton']
            report['cloudViewports'] = []
            for width, height in [(900, 700), (360, 640)]:
                page.set_viewport_size({'width': width, 'height': height})
                expect(dialog.locator('[data-cloud-retranslation-ready]')).to_be_in_viewport(ratio=1)
                sample = geometry(run)
                assert sample['fullyVisible'] and sample['centerHitsButton']
                report['cloudViewports'].append({'width': width, 'height': height, 'submit': sample})
                page.screenshot(path=str(args.report.with_name(args.report.stem + f'-cloud-{width}x{height}.png')))
            page.set_viewport_size({'width': 900, 'height': 700})
            report['checks'].append('Cloud switch retains the edited local comparison and entire original; visible instructions precede any paid submission')
            engine = dialog.get_by_role('combobox', name='Translation method', exact=True)
            engine.select_option('local')
            expect(output).to_have_value(edited)
            engine.select_option('cloud')
            run.click()
            page.wait_for_function('window.__workspacePending.length===1')
            assert page.evaluate("window.__workspaceCalls.filter(x=>x.cmd==='try_translation_text').length") == 1
            page.evaluate('window.__workspacePending.shift()()')
            expect(output).to_have_value('Cloud result from the original.')
            expect(dialog.locator('[data-cloud-retranslation-ready]')).to_have_count(0)
            report['checks'].append('Switching back restores exact local edits; only explicit Translate makes one cloud request')
            output.fill('Reviewed cloud reply — 保留原文与条件。')
            dialog.get_by_label('Draft name', exact=True).fill('Visibility regression draft')
            dialog.get_by_role('button', name='Save draft', exact=True).click()
            saved_link = dialog.get_by_role('button', name='Open this saved draft', exact=True)
            expect(saved_link).to_be_visible()
            saved_link.scroll_into_view_if_needed()
            saved_scroll = dialog.locator('[data-workspace-scroll]').evaluate('e=>e.scrollTop')
            saved_link.click()
            library = page.get_by_role('dialog', name='Saved drafts', exact=True)
            expect(library.get_by_role('textbox', name='Saved text', exact=True)).to_have_value('Reviewed cloud reply — 保留原文与条件。')
            library.get_by_role('button', name='Close', exact=True).click()
            expect(output).to_have_value('Reviewed cloud reply — 保留原文与条件。')
            expect(entry).to_have_value(original)
            actual_scroll = dialog.locator('[data-workspace-scroll]').evaluate('e=>e.scrollTop')
            assert abs(actual_scroll - saved_scroll) <= 2, (saved_scroll, actual_scroll)
            assert geometry(dialog.get_by_role('button', name='Close', exact=True))['centerHitsButton']
            report['checks'].append('Named exact edited save/retrieval and nested-dialog return preserve original, result and workspace scroll; Close stays reachable')
            page.screenshot(path=str(args.report.with_suffix('.png')))
            dialog.get_by_role('button', name='Close', exact=True).click()
            expect(page.locator('dialog.ol-style-trial[open]')).to_have_count(0)
            report['passed'] = True
    except Exception as error:
        report['failure'] = str(error)
        page.screenshot(path=str(args.report.with_name(args.report.stem + '-failure.png')))
        raise
    finally:
        browser.close()
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
print(json.dumps({'measured': len(report['viewports']), 'allActionsVisible': report.get('allMeasuredActionsVisibleAndHitTestable'), 'checks': len(report['checks']), 'passed': report['passed'], 'measureOnly': args.measure_only}))
