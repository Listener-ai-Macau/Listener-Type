"""Keep a selected local recording through speech setup, including failed/late saves.
Actual React in a headless browser; controlled credential/recognition replies only.
No native vault, real service, microphone, clipboard or daily desktop operation.
"""
import argparse
import hashlib
import io
import json
import re
import wave
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', required=True, type=Path)
p.add_argument('--expect-loss', action='store_true')
a = p.parse_args()
sample = io.BytesIO()
with wave.open(sample, 'wb') as wav:
    wav.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
    wav.writeframes(b'\0\0' * 1600)
audio_bytes = sample.getvalue()
checks = []
report = {'scope': __doc__, 'passed': False, 'checks': checks}
fixture_js = """
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
window.__speechCalls=[];window.__speechReady=false;
window.__speechStored='existing-resource';
window.__speechFixture=(cmd,args,mock)=>{
 window.__speechCalls.push({cmd,args});
 if(cmd==='get_credentials')return {...mock(),activeAsrProvider:'volcengine',asrConfigured:window.__speechReady};
 if(cmd==='read_credential')return args.account==='volcengine.resource_id'?window.__speechStored:null;
 if(cmd==='set_credential')return new Promise((resolve,reject)=>{
  window.__speechFinish=()=>{window.__speechStored=args.value;window.__speechReady=true;resolve();};
  window.__speechFail=()=>reject(Error('controlled write failure'));
 });
 if(cmd==='transcribe_audio_file_trial')return {text:'Recognized original, not reference words.',audioDurationMs:100,elapsedMs:20};
 return mock();
};
"""
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge')
    try:
        context = browser.new_context(viewport={'width': 1280, 'height': 1000})
        context.route(re.compile(r'^https?://'), lambda r: r.continue_()
            if re.match(r'^https?://(localhost|127\.0\.0\.1)(:|/)', r.request.url) else r.abort())
        page = context.new_page()
        page.set_default_timeout(6500)
        page.add_init_script(fixture_js)
        def fixture(route):
            response = route.fetch()
            source = response.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert source.count(old) == 1
            route.fulfill(response=response, body=source.replace(old,
                'if (!isTauri) {\n    return window.__speechFixture(cmd,args,mock);\n  }', 1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), fixture)
        page.goto(a.url, wait_until='networkidle')
        page.get_by_role('button', name='Transcribe recorded audio', exact=True).click()
        audio = page.get_by_role('dialog', name='Transcribe recorded audio', exact=True)
        audio.get_by_label('Choose WAV file', exact=True).set_input_files({
            'name': 'my-recording-条件.wav', 'mimeType': 'audio/wav', 'buffer': audio_bytes})
        expect(audio.get_by_text('my-recording-条件.wav', exact=True)).to_be_visible()
        expect(audio.get_by_role('button', name='Transcribe', exact=True)).to_be_disabled()
        audio.get_by_role('button', name='Speech settings', exact=True).click()
        if a.expect_loss:
            expect(audio).to_have_count(0)
            page.get_by_title('Close', exact=True).click()
            page.get_by_role('button', name='Transcribe recorded audio', exact=True).click()
            expect(audio.get_by_text('No audio selected.', exact=True)).to_be_visible()
            checks.append('Before: settings unmounts the recording window; reopening loses the selected local file')
            report['expectedFailureReproduced'] = True
        else:
            settings = page.get_by_role('dialog', name='Speech settings', exact=True)
            expect(settings).to_be_visible()
            expect(audio).to_have_count(1)
            expect(settings.get_by_text('ASR (transcription)', exact=True)).to_be_visible()
            expect(settings.get_by_text('LLM (polishing)', exact=True)).to_have_count(0)
            field = settings.get_by_placeholder('volc.bigasr.sauc.duration', exact=True)
            expect(field).to_have_value('existing-resource')
            page.clock.install()
            field.fill('failed-resource')
            field.press('Escape')
            page.wait_for_function("window.__speechCalls.filter(x=>x.cmd==='set_credential').length===1")
            page.evaluate('window.__speechFail()')
            expect(settings.get_by_role('alert')).to_contain_text('Some settings could not be saved.')
            expect(field).to_have_value('failed-resource')
            settings.get_by_role('button', name='Return to my recording', exact=True).click()
            expect(audio.get_by_text('my-recording-条件.wav', exact=True)).to_be_visible()
            expect(audio.get_by_role('button', name='Transcribe', exact=True)).to_be_disabled()
            checks.append('Failed settings remain explicit; deliberate return keeps selection without claiming setup succeeded')
            audio.get_by_role('button', name='Speech settings', exact=True).click()
            expect(field).to_have_value('existing-resource')
            field.fill('successful-resource')
            field.press('Escape')
            page.wait_for_function("window.__speechCalls.filter(x=>x.cmd==='set_credential').length===2")
            expect(settings.get_by_role('button', name='Saving…', exact=True)).to_be_disabled()
            assert page.evaluate("window.__speechCalls.filter(x=>x.cmd==='transcribe_audio_file_trial').length") == 0
            page.evaluate('window.__speechFinish()')
            expect(settings).not_to_be_visible()
            expect(audio.get_by_text('my-recording-条件.wav', exact=True)).to_be_visible()
            expect(audio.get_by_role('button', name='Transcribe', exact=True)).to_be_enabled()
            checks.append('Escape flushes and awaits the actual save before readiness refresh; same recording retained, no upload')
            audio.get_by_role('button', name='Transcribe', exact=True).click()
            expect(audio.get_by_role('textbox', name='Transcript', exact=True)).to_have_value('Recognized original, not reference words.')
            calls = page.evaluate('window.__speechCalls')
            sent = [c for c in calls if c['cmd'] == 'transcribe_audio_file_trial']
            assert len(sent) == 1 and bytes(sent[0]['args']['wavBytes']) == audio_bytes
            assert all(c['args']['account'] == 'volcengine.resource_id' for c in calls if c['cmd'] == 'set_credential')
            assert not any(c['cmd'] in {'set_active_asr_provider','set_active_llm_provider','update_preferences','start_dictation','save_text_draft','insert_style_trial_result'} for c in calls)
            checks.append('Only explicit transcription sends the exact original bytes once; only deliberate speech field writes, no provider/default/device or live capture mutation')
        report.update(passed=True, audioSha256=hashlib.sha256(audio_bytes).hexdigest())
        a.report.parent.mkdir(parents=True, exist_ok=True)
        a.report.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
        print(json.dumps({'passed':True,'checks':checks},ensure_ascii=False))
    finally:
        browser.close()
