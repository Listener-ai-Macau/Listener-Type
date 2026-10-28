"""Actual React multi-step input retention, source retranslation and audio discovery.

Private headless browser, controlled IPC/service results and browser mock draft
storage. No real cloud, audio, clipboard, models, hardware or owner desktop.
Native storage and ordinary MSI verification remain separate required evidence.
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
args = parser.parse_args()
args.report.parent.mkdir(parents=True, exist_ok=True)
report = {'at': datetime.datetime.now().astimezone().isoformat(), 'scope': __doc__, 'checks': [], 'passed': False}
original = 'Hi Mei, do not send the revised deck before 9 November 2026. Keep all 12 people and the $350 budget.\nThanks for the last-minute update.'
formal = 'Dear Mei,\nPlease do not send the revised deck before 9 November 2026. Retain the 12 attendees and the $350 budget.\nThank you for the last-minute update.'
local = '请不要提前发送修改后的甲板。保留12人和350美元预算。'
polished = '请勿提前发送修改后的甲板。请保留12人及350美元的预算。'
edited = '请勿在2026年11月9日之前发送修订后的演示稿。请保留12位参与者及350美元预算。\n感谢临时通知。'

with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge', args=['--disable-gpu'])
    page = browser.new_page(viewport={'width': 1000, 'height': 760})
    try:
        page.add_init_script("""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.__sourceCalls=[]; window.__sourceCloud=[];
window.__sourceFixture=(cmd,args,mock)=>{
 window.__sourceCalls.push({cmd,args});
 if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:true};
 if(cmd==='get_local_translation_status')return {supported:true,runtimeReady:true,runtimeDownloadBytes:0,pairs:[{source:'en',target:'zh',ready:true,downloadBytes:70743021},{source:'zh',target:'en',ready:true,downloadBytes:74481402}],preparing:false,receivedBytes:0,expectedBytes:0};
 if(cmd==='try_style_pack_text')return {packId:args.stylePackId,packName:'Formal',outputText:window.__sourceReplies[args.rawText]??args.rawText,usedTextService:true,elapsedMs:350};
 if(cmd==='try_local_translation_text')return {packId:'local_translation:'+args.sourceLanguage+':'+args.targetLanguage,packName:'Local',outputText:window.__localOutput,usedTextService:false,elapsedMs:400};
 if(cmd==='try_translation_text')return new Promise(resolve=>window.__sourceCloud.push(()=>resolve({packId:'translation:'+args.targetLanguage,packName:'Cloud',outputText:'从完整英文原文重新翻译的结果。',usedTextService:true,elapsedMs:500})));
 return mock();
};
""")

        def fixture(route):
            response = route.fetch()
            source = response.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert source.count(old) == 1
            route.fulfill(response=response, body=source.replace(old, 'if (!isTauri) {\n    return window.__sourceFixture(cmd,args,mock);\n  }', 1))

        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), fixture)
        page.goto(args.url, wait_until='networkidle')
        # This fixture tests continuation/storage, not an omitted-date result.
        # A second rewrite of the manually corrected full draft must retain its
        # date/quantity facts rather than reuse an older, shorter fixture reply.
        page.evaluate('([original,formal,local,polished,edited])=>{window.__sourceReplies={[original]:formal,[local]:polished,[edited]:edited};window.__localOutput=local}', [original, formal, local, polished, edited])
        audio = page.get_by_role('button', name='Transcribe recorded audio', exact=True)
        expect(audio).to_be_visible()
        assert audio.evaluate("e=>!e.closest('details:not([open])')")
        audio.click()
        audio_dialog = page.get_by_role('dialog', name='Transcribe recorded audio', exact=True)
        expect(audio_dialog.get_by_label('Choose WAV file', exact=True)).to_be_visible()
        audio_dialog.get_by_role('button', name='Close', exact=True).click()
        report['checks'].append('Overview exposes the actual existing WAV-import dialog without opening More tools or sending audio')

        page.get_by_role('button', name='Open text workspace', exact=True).click()
        dialog = page.get_by_role('dialog', name='Text workspace', exact=True)
        entry = dialog.get_by_label('Your text', exact=True)
        entry.fill(original)
        dialog.locator('select:has(option[value="builtin.formal"])').select_option('builtin.formal')
        dialog.get_by_role('button', name='Try this text', exact=True).click()
        output = dialog.get_by_label('Result text', exact=True)
        expect(output).to_have_value(formal)
        dialog.get_by_role('button', name='Translate this result…', exact=True).click()
        dialog = page.locator('dialog.ol-style-trial[open]')
        expect(dialog.get_by_label('Your text', exact=True)).to_have_value(formal)
        expect(dialog.locator('[data-workspace-original] pre')).to_have_text(original)
        dialog.get_by_role('combobox', name='Translate output to', exact=True).select_option('简体中文')
        dialog.get_by_role('combobox', name='Original language', exact=True).select_option('en')
        dialog.get_by_role('button', name='Translate', exact=True).click()
        expect(dialog.get_by_label('Result text', exact=True)).to_have_value(local)
        dialog.get_by_role('button', name='Refine this translation…', exact=True).click()
        dialog = page.get_by_role('dialog', name='Text workspace', exact=True)
        expect(dialog.get_by_label('Your text', exact=True)).to_have_value(local)
        dialog.get_by_role('button', name='Try this text', exact=True).click()
        output = dialog.get_by_label('Result text', exact=True)
        expect(output).to_have_value(polished)
        output.fill(edited)
        dialog.get_by_label('Draft name', exact=True).fill('Reviewed customer notice')
        service_calls = page.evaluate("window.__sourceCalls.filter(x=>['try_style_pack_text','try_local_translation_text','try_translation_text'].includes(x.cmd)).length")
        dialog.get_by_role('button', name='Retranslate from the original…', exact=True).click()
        dialog = page.locator('dialog.ol-style-trial[open]')
        expect(dialog.get_by_label('Your text', exact=True)).to_have_value(formal)
        expect(dialog.locator('[data-text-previous-step]')).to_contain_text(edited)
        expect(dialog.locator('[data-workspace-original] pre')).to_have_text(original)
        assert page.evaluate("window.__sourceCalls.filter(x=>['try_style_pack_text','try_local_translation_text','try_translation_text'].includes(x.cmd)).length") == service_calls
        dialog.get_by_role('button', name='Return to my translation draft', exact=True).click()
        dialog = page.get_by_role('dialog', name='Text workspace', exact=True)
        expect(dialog.get_by_label('Your text', exact=True)).to_have_value(local)
        expect(dialog.get_by_label('Result text', exact=True)).to_have_value(edited)
        expect(dialog.get_by_label('Draft name', exact=True)).to_have_value('Reviewed customer notice')
        report['checks'].append('Original → formal English → local Chinese → refined/manual Chinese retains the first input; opening and returning from source retranslation makes no service call and restores exact input/result/name')

        dialog.get_by_role('button', name='Retranslate from the original…', exact=True).click()
        dialog = page.locator('dialog.ol-style-trial[open]')
        expect(dialog.locator('[data-cloud-retranslation-ready]')).to_contain_text('charges')
        for width, height in [(360, 640), (900, 700)]:
            page.set_viewport_size({'width': width, 'height': height})
            for label in ['Translate', 'Return to my translation draft']:
                action = dialog.get_by_role('button', name=label, exact=True)
                assert action.evaluate('e=>{const r=e.getBoundingClientRect(),hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);return r.x>=0&&r.right<=innerWidth&&r.y>=0&&r.bottom<=innerHeight&&!!hit&&e.contains(hit)}'), (width, height, label)
        page.set_viewport_size({'width': 1000, 'height': 760})
        dialog.get_by_role('button', name='Translate', exact=True).click()
        page.wait_for_function('window.__sourceCloud.length===1')
        expect(dialog.get_by_role('button', name='Return to my translation draft', exact=True)).to_be_disabled()
        calls = page.evaluate("window.__sourceCalls.filter(x=>x.cmd==='try_translation_text')")
        assert len(calls) == 1 and calls[0]['args']['rawText'] == formal
        page.evaluate('window.__sourceCloud.shift()()')
        expect(dialog.get_by_label('Result text', exact=True)).to_have_value('从完整英文原文重新翻译的结果。')
        dialog.get_by_role('button', name='Return to my translation draft', exact=True).click()
        dialog = page.get_by_role('dialog', name='Text workspace', exact=True)
        expect(dialog.get_by_label('Result text', exact=True)).to_have_value(edited)
        dialog.get_by_role('button', name='Retranslate from the original…', exact=True).click()
        dialog = page.locator('dialog.ol-style-trial[open]')
        expect(dialog.get_by_label('Result text', exact=True)).to_have_value('从完整英文原文重新翻译的结果。')
        assert page.evaluate("window.__sourceCalls.filter(x=>x.cmd==='try_translation_text').length") == 1
        dialog.get_by_role('button', name='Return to my translation draft', exact=True).click()
        dialog = page.get_by_role('dialog', name='Text workspace', exact=True)
        report['checks'].append('Only explicit Translate submits the complete pre-translation source once; pending state fences return, and both the cloud result and edited refinement remain reusable without another call')

        dialog.get_by_role('button', name='Save draft', exact=True).click()
        saved_link = dialog.get_by_role('button', name='Open this saved draft', exact=True)
        expect(saved_link).to_be_visible()
        writes = page.evaluate("window.__sourceCalls.filter(x=>x.cmd==='save_text_draft')")
        assert len(writes) == 1 and writes[0]['args']['original'] == original and writes[0]['args']['text'] == edited
        saved_link.click()
        library = page.get_by_role('dialog', name='Saved drafts', exact=True)
        expect(library.get_by_role('textbox', name='Saved text', exact=True)).to_have_value(edited)
        expect(library.locator('pre.ol-saved-phrase-text')).to_have_text(original)
        library.get_by_role('button', name='Continue in text workspace…', exact=True).click()
        dialog = page.get_by_role('dialog', name='Text workspace', exact=True)
        expect(dialog.get_by_label('Your text', exact=True)).to_have_value(edited)
        expect(dialog.locator('[data-workspace-original] pre')).to_have_text(original)
        dialog.locator('select:has(option[value="builtin.formal"])').select_option('builtin.formal')
        dialog.get_by_role('button', name='Try this text', exact=True).click()
        dialog.get_by_label('Draft name', exact=True).fill('Further reviewed copy')
        dialog.get_by_role('button', name='Save draft', exact=True).click()
        expect(dialog.get_by_role('button', name='Open this saved draft', exact=True)).to_be_visible()
        writes = page.evaluate("window.__sourceCalls.filter(x=>x.cmd==='save_text_draft')")
        assert len(writes) == 2 and writes[1]['args']['original'] == original and writes[1]['args']['id'] != writes[0]['args']['id']
        report['checks'].append('Explicit final save sends exact first English and edited Chinese; library retrieval and continued editing preserve that original in a separate immutable new record using the unchanged v1 shape')
        page.screenshot(path=str(args.report.with_suffix('.png')))
        dialog.get_by_label('Your text', exact=True).fill('')
        dialog.get_by_label('Your text', exact=True).fill('A separate new draft.')
        expect(dialog.locator('[data-workspace-original]')).to_have_count(0)
        expect(dialog.locator('[data-refinement-original-translation]')).to_have_count(0)
        report['checks'].append('Clearing the input starts a separate draft without retaining an unrelated original or translation route')
        dialog.get_by_role('button', name='Close', exact=True).click()
        page.get_by_role('button', name='Tools', exact=True).click()
        audio_card = page.locator('[data-tool="audio-file"]')
        expect(audio_card).to_be_visible()
        assert audio_card.evaluate("e=>!e.closest('details')")
        audio_card.click()
        expect(page.get_by_role('dialog', name='Transcribe recorded audio', exact=True)).to_be_visible()
        report['checks'].append('Tools exposes audio import as a normal visible card routed to the existing file feature')
        report['passed'] = True
    except Exception as error:
        report['failure'] = str(error)
        page.screenshot(path=str(args.report.with_name(args.report.stem + '-failure.png')))
        raise
    finally:
        browser.close()
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
print(json.dumps({'checks': len(report['checks']), 'passed': report['passed']}))
