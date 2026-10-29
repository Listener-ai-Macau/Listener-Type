"""Actual React failure/history UI with synthetic records and a muted WAV.

The IPC fixture supplies a failed record; it does not test native persistence.
No Owner history, microphone, desktop input, service or physical device.
"""
import argparse
from datetime import datetime
import io
import json
from pathlib import Path
import re
import wave
from playwright.sync_api import expect, sync_playwright

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--url', required=True)
p.add_argument('--report', type=Path, required=True)
a = p.parse_args()
buffer = io.BytesIO()
with wave.open(buffer, 'wb') as f:
    f.setnchannels(1); f.setsampwidth(2); f.setframerate(16000)
    f.writeframes(b'\0\0' * 4000)
wav = buffer.getvalue()
titles = {'en': 'Recording connection failed', 'zh-CN': '录音连接失败',
          'zh-TW': '錄音連線失敗', 'ja': '録音接続エラー', 'ko': '녹음 연결 실패'}
history_labels = {'en': 'History', 'zh-CN': '历史', 'zh-TW': '歷史', 'ja': '履歴', 'ko': '기록'}
export_labels = {'en': 'Export recording', 'zh-CN': '导出录音', 'zh-TW': '匯出錄音',
                 'ja': '録音をエクスポート', 'ko': '녹음 내보내기'}
report = {'startedAt': datetime.now().astimezone().isoformat(), 'scope': __doc__, 'cases': []}
failed_id = 'df19f5be-4d6f-4c59-9567-a142db003701'
with sync_playwright() as pw:
    browser = pw.chromium.launch(headless=True, channel='msedge', args=['--mute-audio'])
    try:
        for locale, title in titles.items():
            for retain in [True, False]:
                context = browser.new_context(viewport={'width': 1280, 'height': 900}, accept_downloads=True)
                page = context.new_page()
                page.set_default_timeout(7000)
                rows = [{'id': failed_id, 'createdAt': '2026-10-29T00:00:00Z',
                         'rawTranscript': '', 'finalText': '', 'mode': 'raw', 'durationMs': 250,
                         'hasAudioRecording': retain, 'errorCode': 'recordingConnectionFailed', 'insertStatus': 'failed'}]
                page.add_init_script("""
localStorage.setItem('ol.locale', LOCALE);
sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
window.__failureReads=[];
window.__failureFixture=(cmd,args,mock)=>{
 if(cmd==='list_history')return ROWS;
 if(cmd==='read_audio_recording'){window.__failureReads.push(args.sessionId);return WAV;}
 return mock();
};
""".replace('LOCALE', json.dumps(locale)).replace('ROWS', json.dumps(rows)).replace('WAV', json.dumps(list(wav))))
                def ipc_fixture(route):
                    response = route.fetch(); source = response.text()
                    old = 'if (!isTauri) {\n    return mock();\n  }'
                    assert source.count(old) == 1
                    route.fulfill(response=response, body=source.replace(old, 'if (!isTauri) {\n    return window.__failureFixture(cmd,args,mock);\n  }', 1))
                page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'), ipc_fixture)
                page.goto(a.url, wait_until='networkidle')
                page.get_by_role('button', name=history_labels[locale], exact=True).click()
                page.get_by_role('button', name=re.compile(re.escape(title))).last.click()
                expect(page.get_by_text(title, exact=True).last).to_be_visible()
                if locale == 'en':
                    expect(page.get_by_role('button', name='Copy', exact=True)).to_be_disabled()
                export = page.get_by_role('button', name=export_labels[locale], exact=True)
                if retain:
                    with page.expect_download() as pending:
                        export.click()
                    download = pending.value
                    data = Path(download.path()).read_bytes()
                    assert data == wav, 'recovered export must contain the exact received audio'
                    assert failed_id in download.suggested_filename
                    assert page.evaluate('window.__failureReads') == [failed_id]
                else:
                    assert page.locator('audio').count() == 0
                    expect(export).to_have_count(0)
                    assert page.evaluate('window.__failureReads') == []
                if locale == 'en' and retain:
                    page.screenshot(path=str(a.report.with_suffix('.png')), full_page=True)
                report['cases'].append({'locale': locale, 'retention': retain,
                                        'failureTitleVisible': True, 'exactWavExport': retain,
                                        'noAudioReadWithoutRetention': not retain})
                context.close()
        report['completed'] = True
    finally:
        browser.close()
        report['finishedAt'] = datetime.now().astimezone().isoformat()
        a.report.parent.mkdir(parents=True, exist_ok=True)
        a.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
print(json.dumps(report, ensure_ascii=False, indent=2))
