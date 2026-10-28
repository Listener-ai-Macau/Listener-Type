"""Actual React audio result/save/return reachability without action autoscroll.

Controlled ASR and browser mock storage only. No real audio service, clipboard,
Owner desktop, GPU inference or hardware. Exact MSI evidence remains separate.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect

p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
a.report.parent.mkdir(parents=True,exist_ok=True)
report={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'scenarios':[],'passed':False}
text='Keep Thursday at three. Do not send the revised file before review. Budget: 50 dollars.\n' * 18
hit="""e=>{const r=e.getBoundingClientRect(),clip={left:0,top:0,right:innerWidth,bottom:innerHeight};for(let p=e.parentElement;p;p=p.parentElement){const s=getComputedStyle(p),b=p.getBoundingClientRect();if(/auto|scroll|hidden|clip/.test(s.overflowY)){clip.top=Math.max(clip.top,b.top);clip.bottom=Math.min(clip.bottom,b.bottom)}if(/auto|scroll|hidden|clip/.test(s.overflowX)){clip.left=Math.max(clip.left,b.left);clip.right=Math.min(clip.right,b.right)}}const h=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);return {x:r.x+r.width/2,y:r.y+r.height/2,fullyVisible:r.top>=clip.top&&r.bottom<=clip.bottom&&r.left>=clip.left&&r.right<=clip.right,ownHit:!!h&&e.contains(h)}}"""
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu'])
 try:
  for width,height,scale in [(1240,800,1.1),(900,700,1),(600,700,1),(360,640,1),(1280,720,1.25)]:
   for expanded in [False,True]:
    page=browser.new_page(viewport={'width':width,'height':height});page.set_default_timeout(6500)
    row={'width':width,'height':height,'scale':scale,'referenceExpanded':expanded,'stages':[],'passed':False}
    report['scenarios'].append(row)
    try:
     page.add_init_script("""
localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');window.__calls=[];
window.__audioResultFixture=(cmd,args,mock)=>{window.__calls.push({cmd,args:cmd==='transcribe_audio_file_trial'?{requestId:args.requestId,wavBytesLength:args.wavBytes.length}:args});
 if(cmd==='get_credentials')return {...mock(),activeAsrProvider:'volcengine',asrConfigured:true};
 if(cmd==='transcribe_audio_file_trial')return new Promise(resolve=>window.__completeASR=()=>resolve({text:window.__transcript,audioDurationMs:11500,elapsedMs:1600}));
 return mock();};
""")
     def fixture(route):
      r=route.fetch();s=r.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1
      route.fulfill(response=r,body=s.replace(old,'if (!isTauri) {\n    return window.__audioResultFixture(cmd,args,mock);\n  }',1))
     page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
     page.goto(a.url,wait_until='networkidle')
     page.evaluate("([s,t])=>{document.documentElement.style.zoom=String(s);document.documentElement.style.setProperty('--ol-font-scale',String(s));window.__transcript=t}",[scale,text])
     page.get_by_role('button',name='Transcribe recorded audio',exact=True).click()
     dialog=page.get_by_role('dialog',name='Transcribe recorded audio',exact=True)
     dialog.get_by_role('button',name='Use included sample',exact=True).click()
     if expanded:dialog.locator('details > summary').first.click()
     dialog.get_by_role('button',name='Transcribe',exact=True).click()
     page.wait_for_function('typeof window.__completeASR==="function"');page.evaluate('window.__completeASR()')
     expect(dialog.get_by_role('textbox',name='Transcript',exact=True)).to_have_value(text)
     def geometry(stage,labels):
      values={name:dialog.get_by_role('button',name=name,exact=True).evaluate(hit) for name in labels}
      row['stages'].append({'stage':stage,'actions':values})
      return values
     def visible(values):return all(x['fullyVisible'] and x['ownHit'] for x in values.values())
     labels=['Copy transcript','Save draft','Open in text workspace','Discard this result and choose again','Close']
     values=geometry('fresh_result_no_action_scroll',labels)
     if not visible(values):
      if width in [1240,360] and expanded:page.screenshot(path=str(a.report.with_name(a.report.stem+'-clipped-'+str(width)+'.png')))
      continue
     save=values['Save draft'];page.mouse.click(save['x'],save['y'])
     expect(dialog.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(1)
     saved_labels=[x for x in labels if x!='Save draft']+['Saved draft','Open this saved draft']
     values=geometry('saved_confirmation_expansion',saved_labels);assert visible(values),values
     assert page.evaluate('window.__calls.filter(x=>x.cmd==="save_text_draft").at(-1).args.original')==text
     open_saved=values['Open this saved draft'];page.mouse.click(open_saved['x'],open_saved['y'])
     library=page.get_by_role('dialog',name='Saved drafts',exact=True)
     expect(library.get_by_role('textbox',name='Saved text',exact=True)).to_have_value(text)
     library.get_by_role('button',name='Close',exact=True).click()
     assert visible(geometry('saved_library_return',saved_labels))
     for position in ['top','bottom']:
      dialog.evaluate("(d,p)=>{const s=d.querySelector('[data-audio-file-scroll]')||d;s.scrollTop=p==='top'?0:s.scrollHeight}",position)
      assert visible(geometry('saved_body_'+position,saved_labels))
     point=geometry('continue_pointer',saved_labels)['Open in text workspace']
     page.mouse.click(point['x'],point['y'])
     expect(page.get_by_role('textbox',name='Your text',exact=True)).to_have_value(text)
     assert not page.evaluate("window.__calls.some(x=>['try_style_pack_text','try_translation_text','try_local_translation_text','start_dictation','insert_style_trial_result','update_preferences'].includes(x.cmd))")
     assert len(page.evaluate("window.__calls.filter(x=>x.cmd==='transcribe_audio_file_trial')"))==1
     row['passed']=True
    except Exception as error:
     row['failure']=repr(error)
     page.screenshot(path=str(a.report.with_name(a.report.stem+'-error-'+str(width)+'.png')))
    finally:page.close()
 finally:
  browser.close();report['passed']=all(x['passed'] for x in report['scenarios']) and len(report['scenarios'])==10
  a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
print(json.dumps({'passed':report['passed'],'scenarios':len(report['scenarios']),'failed':sum(not x['passed'] for x in report['scenarios'])}))
raise SystemExit(0 if report['passed'] else 1)
