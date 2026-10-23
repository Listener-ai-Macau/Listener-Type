"""Actual History UI, controlled delayed recording reads and real Blob lifetimes.

No real user audio, service, device or foreground interaction. Checks releases
of audio resources after switching records or leaving History during a read.
"""
import argparse
from datetime import datetime
import io
import json
from pathlib import Path
import re
import wave

from playwright.sync_api import expect, sync_playwright

p = argparse.ArgumentParser()
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
p.add_argument('--before', action='store_true')
a = p.parse_args()
wav = io.BytesIO()
with wave.open(wav, 'wb') as f:
    f.setnchannels(1); f.setsampwidth(2); f.setframerate(16000)
    f.writeframes(b'\0\0'*4000)
rows = [{'id': id, 'createdAt': '2026-10-23T04:00:00Z', 'rawTranscript': text,
         'finalText': text, 'mode': 'raw', 'durationMs': 250,
         'hasAudioRecording': True, 'errorCode': None, 'insertStatus': 'copiedFallback'}
        for id, text in [('a0d7e4cb-1559-478e-b221-3d04db809ead', 'Recording A fixture'),
                         ('b106dfbe-715b-4c4f-a940-5c28b27e1b6f', 'Recording B fixture')]]
report = {'startedAt': datetime.now().astimezone().isoformat(), 'before': a.before,
          'scope': __doc__}
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge', args=['--mute-audio'])
    try:
        page = browser.new_page(viewport={'width': 1280, 'height': 900})
        page.set_default_timeout(6000)
        page.add_init_script("""
localStorage.setItem('ol.locale','en');
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
window.__audioReads=[];window.__blobCreated=[];window.__blobRevoked=[];
const create=URL.createObjectURL.bind(URL),revoke=URL.revokeObjectURL.bind(URL);
URL.createObjectURL=b=>{const u=create(b);window.__blobCreated.push(u);return u;};
URL.revokeObjectURL=u=>{window.__blobRevoked.push(u);revoke(u);};
window.__audioFixture=(cmd,args,mock)=>{
 if(cmd==='list_history')return ROWS;
 if(cmd==='read_audio_recording')return new Promise((resolve,reject)=>window.__audioReads.push({id:args.sessionId,resolve,reject}));
 return mock();
};
""".replace('ROWS', json.dumps(rows)))
        def fixture(route):
            r = route.fetch(); source = r.text()
            old = 'if (!isTauri) {\n    return mock();\n  }'
            assert source.count(old) == 1
            route.fulfill(response=r, body=source.replace(old, 'if (!isTauri) {\n    return window.__audioFixture(cmd,args,mock);\n  }', 1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), fixture)
        page.goto(a.url, wait_until='networkidle')
        page.get_by_role('button', name='History', exact=True).click()
        def select(name): page.get_by_role('button', name=re.compile(re.escape(name))).last.click()
        def play(): page.get_by_role('button', name='Play recording', exact=True).click()
        def resolve(index):
            page.evaluate('(x)=>window.__audioReads[x.index].resolve(x.bytes)', {'index': index, 'bytes': list(wav.getvalue())})
            page.evaluate('new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')
        def live(): return page.evaluate('window.__blobCreated.filter(x=>!window.__blobRevoked.includes(x)).length')
        select('Recording A fixture'); play()
        page.wait_for_function('window.__audioReads.length===1')
        select('Recording B fixture')
        expect(page.get_by_role('button', name='Play recording', exact=True)).to_be_enabled()
        resolve(0)
        report['createdAfterDisposedRead'] = page.evaluate('window.__blobCreated.length')
        report['liveAfterDisposedRead'] = live()
        if a.before:
            assert report['createdAfterDisposedRead'] == 1 and report['liveAfterDisposedRead'] == 1
            expect(page.locator('audio')).to_have_count(0)
            report['reproducedOrphanedBlobForOldRecording'] = True
        else:
            assert report['createdAfterDisposedRead'] == 0 and live() == 0
            play(); page.wait_for_function('window.__audioReads.length===2'); resolve(1)
            expect(page.locator('audio')).to_have_count(1)
            page.wait_for_function('document.querySelector("audio")?.readyState>=1')
            assert page.locator('audio').evaluate('(e)=>e.duration') == 0.25 and live() == 1
            select('Recording A fixture')
            expect(page.locator('audio')).to_have_count(0)
            assert live() == 0
            report['CurrentRecordingPlaysAndReleasesOnSwitch'] = True
            play(); page.wait_for_function('window.__audioReads.length===3')
            page.get_by_role('button', name='Tools', exact=True).click()
            # FloatingShell keeps the old page through its exit animation.
            # Resolve after actual unmount, not while History still owns the read.
            expect(page.get_by_role('heading', name='Tools', exact=True)).to_be_visible()
            resolve(2)
            report['liveAfterLeavingHistory'] = live()
            report['createdAfterLeavingHistory'] = page.evaluate('window.__blobCreated.length')
            assert live() == 0 and page.evaluate('window.__blobCreated.length') == 1
            report['LeavingHistoryDuringReadCannotCreateAnOrphan'] = True
        report['passed'] = True
    except Exception as e:
        report.update(passed=False, error=str(e), errorType=type(e).__name__)
    finally:
        browser.close()
report['finishedAt'] = datetime.now().astimezone().isoformat()
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
print(json.dumps(report, ensure_ascii=False))
raise SystemExit(0 if report['passed'] else 1)
