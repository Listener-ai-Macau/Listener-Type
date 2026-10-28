"""Fresh-result mouse reachability without automatic locator scrolling.

Actual headless React, controlled IPC/local save; no real model, paid service,
system clipboard, Owner desktop or hardware. Native MSI evidence is separate.
"""
import argparse
import datetime
import json
import re
from pathlib import Path
from playwright.sync_api import expect, sync_playwright

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--url',required=True)
p.add_argument('--report',type=Path,required=True)
a=p.parse_args()
a.report.parent.mkdir(parents=True,exist_ok=True)
report={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':False,'scenarios':[]}
original='Project update: keep all 17 links, three screenshots and both owners. If legal does not reply by noon, leave the privacy page unchanged. The search issue is not reproduced; do not describe it as fixed.'
local='项目更新：保留17个链接和三张截图。法务未回复时，不改隐私页。搜索问题尚未复现，不得标为已修复。'
hit="""e=>{const r=e.getBoundingClientRect(),s=e.closest('[data-workspace-scroll]').getBoundingClientRect(),h=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);return {x:r.x+r.width/2,y:r.y+r.height/2,fullyVisible:r.top>=s.top&&r.bottom<=s.bottom&&r.left>=0&&r.right<=innerWidth,ownHit:!!h&&e.contains(h)}}"""

with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu'])
 try:
  for width,height,scale in [(1240,800,1.1),(900,700,1),(600,700,1),(360,640,1),(1280,720,1.25)]:
   for mode in ['raw','local']:
    page=browser.new_page(viewport={'width':width,'height':height})
    page.add_init_script("""
localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');window.__calls=[];
window.__resultFixture=(cmd,args,mock)=>{window.__calls.push({cmd,args});
 if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:true};
 if(cmd==='get_local_translation_status')return {supported:true,runtimeReady:true,runtimeDownloadBytes:0,pairs:[{source:'en',target:'zh',ready:true,downloadBytes:70743021}],preparing:false,receivedBytes:0,expectedBytes:0};
 if(cmd==='try_style_pack_text')return {packId:args.stylePackId,packName:'Raw',outputText:args.rawText,usedTextService:false,elapsedMs:1};
 if(cmd==='try_local_translation_text')return {packId:'local_translation:'+args.sourceLanguage+':'+args.targetLanguage,packName:'Local',outputText:window.__localResult,usedTextService:false,elapsedMs:10};
 return mock();};
""")
    def fixture(route):
     response=route.fetch();source=response.text();old='if (!isTauri) {\n    return mock();\n  }'
     assert source.count(old)==1
     route.fulfill(response=response,body=source.replace(old,'if (!isTauri) {\n    return window.__resultFixture(cmd,args,mock);\n  }',1))
    page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
    page.goto(a.url,wait_until='networkidle')
    page.evaluate("([s,text])=>{document.documentElement.style.zoom=String(s);document.documentElement.style.setProperty('--ol-font-scale',String(s));window.__localResult=text}",[scale,local])
    page.get_by_role('button',name='Open text workspace' if mode=='raw' else 'Chinese ↔ English on this PC',exact=True).click()
    dialog=page.locator('dialog.ol-style-trial[open]')
    dialog.get_by_label('Your text',exact=True).fill(original)
    if mode=='raw':
     dialog.locator('select:has(option[value="builtin.raw"])').select_option('builtin.raw')
    else:
     dialog.get_by_role('combobox',name='Translate output to',exact=True).select_option('简体中文')
     dialog.get_by_role('combobox',name='Original language',exact=True).select_option('en')
    dialog.get_by_role('button',name='Try this text' if mode=='raw' else 'Translate',exact=True).click()
    expect(dialog.get_by_label('Result text',exact=True)).to_have_value(original if mode=='raw' else local)
    names=['Copy result','Save draft','Translate this result…' if mode=='raw' else 'Refine this translation…']
    if mode=='local':names.append('Switch to my cloud service…')
    # Reading geometry and raw mouse clicks do not scroll a target into view.
    # This assertion fails on217 before any saving, focus change or manual wheel.
    for name in names:
     button=dialog.get_by_role('button',name=name,exact=True)
     page.wait_for_function("([label])=>{const b=[...document.querySelectorAll('dialog[open] button')].find(e=>e.innerText.trim()===label);if(!b)return false;const r=b.getBoundingClientRect(),s=b.closest('[data-workspace-scroll]').getBoundingClientRect(),h=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);return r.top>=s.top&&r.bottom<=s.bottom&&!!h&&b.contains(h)}",arg=[name])
    geometry={name:dialog.get_by_role('button',name=name,exact=True).evaluate(hit) for name in names}
    for name in ['Close','Try this text' if mode=='raw' else 'Translate']:
     assert dialog.get_by_role('button',name=name,exact=True).evaluate("e=>{const r=e.getBoundingClientRect(),h=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);return !!h&&e.contains(h)}")
    if (width,scale)==(1240,1.1):page.screenshot(path=str(a.report.with_name(a.report.stem+'-'+mode+'.png')))
    target='Save draft' if mode=='raw' else 'Switch to my cloud service…'
    center=geometry[target];page.mouse.click(center['x'],center['y'])
    if mode=='raw':
     link=dialog.get_by_role('button',name='Open this saved draft',exact=True);expect(link).to_be_visible();link.click()
     library=page.get_by_role('dialog',name='Saved drafts',exact=True)
     expect(library.get_by_role('textbox',name='Saved text',exact=True)).to_have_value(original)
     library.get_by_role('button',name='Close',exact=True).click()
     expect(dialog.get_by_label('Result text',exact=True)).to_have_value(original)
    else:
     expect(dialog.locator('[data-cloud-retranslation-ready]')).to_be_visible()
     expect(dialog.get_by_label('Your text',exact=True)).to_have_value(original)
     expect(dialog.locator('[data-text-previous-step]')).to_contain_text(local)
     assert page.evaluate("window.__calls.filter(x=>x.cmd==='try_translation_text').length")==0
    report['scenarios'].append({'width':width,'height':height,'scale':scale,'mode':mode,'freshActions':geometry,'rawMouseAction':target,'passed':True})
    page.close()
  report['passed']=True
 except Exception as e:
  report['failure']=repr(e)
  page.screenshot(path=str(a.report.with_name(a.report.stem+'-failure.png')))
  raise
 finally:
  browser.close();a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps({'passed':report['passed'],'scenarios':len(report['scenarios'])}))
