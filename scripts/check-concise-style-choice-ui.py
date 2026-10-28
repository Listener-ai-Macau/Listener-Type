"""Direct concise purpose: actual production React with controlled storage/service.
Checks paid-request boundaries, preserved edits, cache and error recovery.
Controlled text is not evidence of real language quality or native hardware.
"""
import argparse, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', required=True, type=Path)
a = p.parse_args()
checks = []
source = "Hi Sam, I need to move tomorrow's 10 o'clock meeting to Friday at 2 because the draft is not ready. Please confirm by 5 today or suggest a time next week."
output = "Hi Sam, tomorrow's 10 o'clock meeting needs to move to Friday at 2 because the draft is not ready. Please confirm by 5 today or suggest a time next week."
with sync_playwright() as pw:
    b = pw.chromium.launch(headless=True, channel='msedge', args=['--disable-gpu'])
    try:
        page = b.new_page(viewport={'width': 1240, 'height': 800})
        page.set_default_timeout(6500)
        page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');window.__calls=[];
        window.__fixture=(cmd,args,mock)=>{
          window.__calls.push({cmd,args});
          if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:!window.__keyless};
          if(cmd==='create_style_pack_from_template'){
            if(window.__failAdd)return Promise.reject(Error('controlled storage failure'));
            return new Promise(resolve=>window.__finishAdd=()=>{window.__finishAdd=null;resolve(mock());});
          }
          if(cmd==='try_style_pack_text'){
            if(args.stylePackId==='builtin.raw')return {packId:args.stylePackId,packName:'Original',outputText:args.rawText,usedTextService:false,elapsedMs:0};
            if(window.__failService)return Promise.reject(Error('controlled request failure'));
            const result={packId:args.stylePackId,packName:'Controlled concise result',outputText:OUTPUT,usedTextService:true,elapsedMs:900};
            if(window.__holdService)return new Promise(resolve=>window.__finishService=()=>resolve(result));
            return result;
          }
          return mock();
        };""".replace('OUTPUT', json.dumps(output)))
        def route_fixture(route):
            response = route.fetch()
            s = response.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert s.count(old) == 1
            route.fulfill(response=response, body=s.replace(old, 'if (!isTauri) {\n    return window.__fixture(cmd,args,mock);\n  }', 1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), route_fixture)
        page.goto(a.url, wait_until='networkidle')
        def open_trial():
            page.get_by_role('button', name='Open text workspace', exact=True).click()
            return page.locator('dialog').last
        def calls(cmd):
            return page.evaluate('window.__calls').copy() if cmd is None else [x for x in page.evaluate('window.__calls') if x['cmd'] == cmd]
        def finish_add():
            page.wait_for_function('!!window.__finishAdd')
            page.evaluate('window.__finishAdd()')
        def fresh():
            page.reload(wait_until='networkidle')
            return open_trial()
        trial = open_trial()
        picker = trial.get_by_role('combobox', name='Trial style', exact=True)
        expect(picker).to_have_value('builtin.raw')
        short = trial.get_by_role('button', name='Make shorter', exact=True)
        expect(short).to_be_disabled()
        trial.get_by_label('Your text', exact=True).fill(source)
        expect(short).to_be_in_viewport()
        expect(short).to_be_enabled()
        trial.get_by_role('button', name='Try this text', exact=True).click()
        editor = trial.get_by_label('Result text', exact=True)
        expect(editor).to_have_value(source)
        editor.fill(source + ' Keep my edit.')
        short.click()
        page.wait_for_function('!!window.__finishAdd')
        expect(picker).to_be_disabled()
        expect(trial.get_by_label('Your text', exact=True)).to_be_disabled()
        expect(trial.get_by_role('button', name='Processing…', exact=True)).to_be_disabled()
        expect(trial.get_by_role('button', name='Shortening…', exact=True)).to_be_disabled()
        page.keyboard.press('Enter')
        assert len(calls('create_style_pack_from_template')) == 1 and len(calls('try_style_pack_text')) == 1
        finish_add()
        expect(editor).to_have_value(output)
        expect(picker).not_to_have_value('builtin.raw')
        expect(trial.locator('[data-style-trial-restored]')).to_have_count(0)
        concise_id = picker.input_value()
        assert len(calls('try_style_pack_text')) == 2
        assert calls('try_style_pack_text')[-1]['args'] == {'stylePackId': concise_id, 'rawText': source}
        created = calls('create_style_pack_from_template')
        assert created[0]['args']['trialOnly'] is True and not created[0]['args']['template']['active']
        assert not any(x['cmd'] in {'set_active_style_pack', 'update_preferences', 'set_credential', 'start_dictation'} for x in calls(None))
        checks.append('One explicit Make shorter prepares/reuses the inactive trial preset and submits exactly once; its fresh result is not labelled as restored/no new request, with no daily activation or competing submit')
        expect(trial).to_contain_text(source + ' Keep my edit.')
        editor.fill(output + ' Edited final.')
        trial.get_by_role('button', name='Save draft', exact=True).click()
        expect(trial.get_by_role('button', name='Saved draft', exact=True)).to_be_disabled()
        assert calls('save_text_draft')[-1]['args']['original'] == source
        assert calls('save_text_draft')[-1]['args']['text'] == output + ' Edited final.'
        picker.select_option('builtin.raw')
        expect(editor).to_have_value(source + ' Keep my edit.')
        trial.get_by_role('button', name='Make shorter', exact=True).click()
        expect(editor).to_have_value(output + ' Edited final.')
        expect(trial.locator('[data-style-trial-restored]')).to_contain_text('no new service request')
        assert len(calls('try_style_pack_text')) == 2 and len(calls('create_style_pack_from_template')) == 1
        checks.append('Exact completed shorter result and its local edits reused with zero further request; original edited result, comparisons and explicit Save survive')
        page.evaluate('window.__failService=true')
        trial.get_by_role('button', name='Try this text', exact=True).click()
        expect(trial.get_by_role('alert')).to_contain_text('controlled request failure')
        expect(editor).to_have_value(output + ' Edited final.')
        expect(trial.locator('[data-style-trial-restored]')).to_have_count(0)
        assert len(calls('try_style_pack_text')) == 3
        page.evaluate('window.__failService=false')
        checks.append('Failed explicit rerun preserves the prior edited body and visible error, without falsely claiming that no new service request was made')
        trial.get_by_label('Your text', exact=True).fill(source + ' New draft.')
        trial.get_by_role('button', name='Make shorter', exact=True).click()
        expect(editor).to_have_value(output)
        expect(trial.locator('[data-style-trial-restored]')).to_have_count(0)
        assert len(calls('try_style_pack_text')) == 4 and calls('try_style_pack_text')[-1]['args']['rawText'] == source + ' New draft.'
        assert len(calls('create_style_pack_from_template')) == 1
        checks.append('Changed source cannot reuse an older shorter result; one new explicit request and no duplicate preset')
        picker.select_option('builtin.raw')
        trial.get_by_label('Your text', exact=True).fill(source + ' Another draft.')
        trial.get_by_role('button', name='Make shorter', exact=True).click()
        expect(editor).to_have_value(output)
        expect(trial.locator('[data-style-trial-restored]')).to_have_count(0)
        assert len(calls('try_style_pack_text')) == 5
        checks.append('Fresh shorter result for a changed draft remains fresh when switching to an already-created target style')
        trial.get_by_role('button', name='Close', exact=True).click()
        trial = open_trial()
        expect(trial.get_by_role('combobox', name='Trial style', exact=True)).to_have_value('builtin.raw')
        assert len(calls('create_style_pack_from_template')) == 1
        checks.append('Reopening keeps the saved inactive preset without silently changing the daily/default selection')

        trial = fresh()
        trial.get_by_label('Your text', exact=True).fill(source)
        trial.get_by_role('button', name='Try this text', exact=True).click()
        page.evaluate('window.__failAdd=true')
        trial.get_by_role('button', name='Make shorter', exact=True).click()
        expect(trial.get_by_role('alert')).to_contain_text('Your draft is kept')
        expect(trial.get_by_label('Your text', exact=True)).to_have_value(source)
        expect(trial.get_by_label('Result text', exact=True)).to_have_value(source)
        expect(trial.get_by_role('combobox', name='Trial style', exact=True)).to_have_value('builtin.raw')
        assert len(calls('try_style_pack_text')) == 1
        checks.append('Failed preset save retains full source, selected style and previous result; no paid request')
        page.evaluate('window.__failAdd=false;window.__failService=true')
        trial.get_by_role('button', name='Make shorter', exact=True).click()
        finish_add()
        expect(trial.get_by_role('alert')).to_contain_text('controlled request failure')
        expect(trial.get_by_label('Result text', exact=True)).to_have_value(source)
        expect(trial.get_by_role('combobox', name='Trial style', exact=True)).to_have_value('builtin.raw')
        page.evaluate('window.__failService=false')
        trial.get_by_role('button', name='Make shorter', exact=True).click()
        expect(trial.get_by_label('Result text', exact=True)).to_have_value(output)
        assert len(calls('create_style_pack_from_template')) == 2  # rejected then successful; retry reuses saved preset
        assert len(calls('try_style_pack_text')) == 3  # Raw, failed service, explicit successful retry
        checks.append('Failed service preserves prior output and draft; explicit retry reuses the already saved preset and makes one request')

        trial = fresh()
        trial.get_by_label('Your text', exact=True).fill(source)
        trial.get_by_role('button', name='Make shorter', exact=True).click()
        page.wait_for_function('!!window.__finishAdd')
        trial.get_by_role('button', name='Close', exact=True).click()
        finish_add()
        expect(page.locator('dialog[open]')).to_have_count(0)
        assert len(calls('try_style_pack_text')) == 0
        checks.append('Closing during preparation prevents starting a paid request after the workspace is gone')

        page.reload(wait_until='networkidle')
        page.evaluate('window.__keyless=true')
        trial = open_trial()
        trial.get_by_label('Your text', exact=True).fill(source)
        expect(trial.get_by_role('button', name='Make shorter', exact=True)).to_be_disabled()
        expect(trial.get_by_role('button', name='Try this text', exact=True)).to_be_enabled()
        assert len(calls('create_style_pack_from_template')) == 0 and len(calls('try_style_pack_text')) == 0
        checks.append('No text service: purpose action remains unavailable while Original text stays usable, with no hidden setup or request')
        a.report.parent.mkdir(parents=True, exist_ok=True)
        a.report.write_text(json.dumps({'passed': True, 'scope': __doc__, 'checks': checks}, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        print(json.dumps({'passed': True, 'checks': checks}))
    finally:
        b.close()
