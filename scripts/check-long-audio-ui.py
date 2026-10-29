"""Controlled browser: long WAV resumability, cancellation and no silent tail loss."""
import argparse,re,struct
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
def make_wav(seconds):
 samples=int(seconds*16000); pcm=bytearray(samples*2)
 for i in range(samples): struct.pack_into('<h',pcm,i*2,(i%30000)-15000)
 return b'RIFF'+struct.pack('<I',36+len(pcm))+b'WAVEfmt '+struct.pack('<IHHIIHH',16,1,1,16000,32000,2,16)+b'data'+struct.pack('<I',len(pcm))+pcm

parser=argparse.ArgumentParser();parser.add_argument('--url',required=True);parser.add_argument('--long-mp3',type=Path);args=parser.parse_args()
with sync_playwright() as p:
 b=p.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu'])
 try:
  page=b.new_page(viewport={'width':1280,'height':1000});page.set_default_timeout(10000)
  page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');window.__calls=[];window.__mode='failure';window.__copies=[];window.__saves=[];Object.defineProperty(navigator,'clipboard',{value:{writeText:text=>{window.__copies.push(text);return Promise.resolve();}}});window.__fixture=(cmd,args,mock)=>{if(cmd==='get_credentials')return {...mock(),activeAsrProvider:'volcengine',asrConfigured:true};if(cmd==='save_text_draft'){window.__saves.push({...args});return mock();}if(cmd==='transcribe_audio_file_trial'){let w=args.wavBytes,i=window.__calls.length;window.__calls.push({id:args.requestId,len:w.length,first:w.slice(44,48),last:w.slice(-4)});if(window.__mode==='failure'&&i===1)throw Error('audio_file_timeout');if(window.__mode==='empty'&&i===1)throw Error('audio_file_empty');if(window.__mode==='cancel'&&i===1)return new Promise((resolve,reject)=>window.__pending={reject,id:args.requestId});return {text:'recognized-'+i,audioDurationMs:(w.length-44)/32,elapsedMs:100};}if(cmd==='cancel_audio_file_trial'){if(window.__pending?.id===args.requestId)window.__pending.reject(Error('audio_file_cancelled'));return;}return mock();};""")
  def fixture(route):
   r=route.fetch();s=r.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1
   route.fulfill(response=r,body=s.replace(old,'if (!isTauri) {\n    return window.__fixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  def open_new(mode):
   page.goto(args.url,wait_until='networkidle');page.evaluate('(v)=>{window.__calls=[];window.__mode=v}',mode)
   page.get_by_role('button',name='Transcribe recorded audio',exact=True).click();d=page.get_by_role('dialog',name='Transcribe recorded audio',exact=True)
   d.get_by_label('Choose WAV file',exact=True).set_input_files({'name':'long.wav','mimeType':'audio/wav','buffer':make_wav(125.004)})
   expect(d.locator('[data-long-audio-plan]')).to_contain_text('3 separate');assert page.evaluate('window.__calls.length')==0
   d.get_by_role('button',name='Transcribe',exact=True).click();return d
  d=open_new('failure');partial=d.locator('[data-audio-file-partial]');expect(partial).to_contain_text('1 of 3 segments processed');expect(d.get_by_role('alert')).to_contain_text('timed out')
  expect(partial.get_by_role('textbox')).to_have_value('[0:00–0:55]\nrecognized-0')
  partial.get_by_role('button',name='Save draft',exact=True).click();expect(partial.get_by_role('button',name='Saved draft',exact=True)).to_be_disabled()
  saved=page.evaluate('window.__saves.at(-1)');assert saved['text']=='[0:00–0:55]\nrecognized-0' and saved['original']==saved['text']
  d.get_by_role('button',name='Close',exact=True).click();expect(d.get_by_role('alert').filter(has_text='Closing ends this transcription')).to_be_visible();d.get_by_role('button',name='Keep transcript open',exact=True).click()
  d.get_by_role('button',name='Continue remaining segments',exact=True).click();result=d.get_by_role('textbox',name='Transcript',exact=True)
  expect(result).to_have_value('[0:00–0:55]\nrecognized-0\n\n[0:54–1:49]\nrecognized-2\n\n[1:48–2:05.004]\nrecognized-3')
  calls=page.evaluate('window.__calls');assert len(calls)==4 and len({x['id'] for x in calls})==4
  assert calls[1]['first']==calls[2]['first'] and calls[0]['first']!=calls[2]['first']
  assert all(c['len']<=55*32000+44 for c in calls) and calls[-1]['last']==list(make_wav(125.004)[-4:])
  d=open_new('cancel');expect(d.locator('[data-audio-file-partial]')).to_contain_text('1 of 3');d.get_by_role('button',name='Stop this request',exact=True).click()
  expect(d.get_by_role('alert')).to_contain_text('Request stopped');d.get_by_role('button',name='Copy completed segments',exact=True).click();assert page.evaluate('window.__copies.at(-1)').endswith('recognized-0')
  d.get_by_role('button',name='Continue remaining segments',exact=True).click();expect(d.get_by_role('textbox',name='Transcript',exact=True)).to_contain_text('recognized-3')
  d=open_new('empty');expect(d.locator('[data-audio-empty-segment]')).to_contain_text('0:54–1:49');d.get_by_role('button',name='Mark this segment unrecognized').click()
  expect(d.locator('[data-audio-file-partial]')).to_contain_text('2 of 3');d.get_by_role('button',name='Continue remaining segments',exact=True).click();expect(d.get_by_role('textbox',name='Transcript',exact=True)).to_contain_text('[No speech recognized; check the original audio]');expect(d.get_by_role('alert')).to_contain_text('marked unrecognized')
  page.goto(args.url,wait_until='networkidle');page.evaluate("window.__calls=[];window.__mode='ok'");page.get_by_role('button',name='Transcribe recorded audio',exact=True).click();d=page.get_by_role('dialog',name='Transcribe recorded audio',exact=True)
  d.get_by_label('Choose WAV file').set_input_files({'name':'boundary.wav','mimeType':'audio/wav','buffer':make_wav(60.5)})
  expect(d.locator('[data-long-audio-plan]')).to_contain_text('2 separate');assert page.evaluate('window.__calls.length')==0
  short=make_wav(60);junk=b'JUNK'+struct.pack('<I',70000)+bytes(70000);with_metadata=short[:12]+junk+short[12:];with_metadata=with_metadata[:4]+struct.pack('<I',len(with_metadata)-8)+with_metadata[8:]
  d.get_by_label('Choose WAV file').set_input_files({'name':'metadata.wav','mimeType':'audio/wav','buffer':with_metadata})
  expect(d.locator('[data-long-audio-plan]')).to_have_count(0);d.get_by_role('button',name='Transcribe',exact=True).click();expect(d.get_by_role('textbox',name='Transcript',exact=True)).to_have_value('recognized-0')
  one=page.evaluate('window.__calls');assert len(one)==1 and one[0]['len']==len(short) and one[0]['last']==list(short[-4:]);d.get_by_role('button',name='Discard this result and choose again').click();page.evaluate('window.__calls=[]')
  if args.long_mp3:
   d.get_by_label('Choose MP3 or M4A').set_input_files(str(args.long_mp3))
   expect(d.locator('[data-long-audio-plan]')).to_contain_text('2 separate');assert page.evaluate('window.__calls.length')==0
  d.get_by_role('button',name='Transcribe',exact=True).click();expect(d.get_by_role('textbox',name='Transcript',exact=True)).to_contain_text('recognized-1');assert page.evaluate('window.__calls.length')==2
  print('PASS 125s WAV: no upload before submit, exact tail, overlap, partial copy, failure/resume without replay, cancel and explicitly marked unrecognized segment')
 finally:b.close()
