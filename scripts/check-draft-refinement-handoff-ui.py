"""Instruction draft -> translation -> refinement uses a body-refinement task.

Actual React, controlled IPC and isolated mock v1 storage. No real service,
model/native storage, microphone, clipboard, hardware or Owner desktop proof.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args();a.report.parent.mkdir(parents=True,exist_ok=True)
brief='Draft a short message to Mia asking her to confirm the review.'
generated='Hi Mia, please confirm the review.'
translated='你好Mia，请确认评审安排。'
manual='Mia，你好。请确认本次评审安排，谢谢。'
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':False,'checks':[]}
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu']);page=b.new_page(viewport={'width':1240,'height':850})
 try:
  page.add_init_script("""
localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');window.__intentCalls=[];
window.__intentFixture=(cmd,args,mock)=>{window.__intentCalls.push({cmd,args});
 if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:args.stylePackId!=='builtin.raw',textServiceConfigured:true};
 if(cmd==='get_local_translation_status')return {supported:true,runtimeReady:true,runtimeDownloadBytes:0,pairs:[{source:'en',target:'zh',ready:true,downloadBytes:0},{source:'zh',target:'en',ready:true,downloadBytes:0}],preparing:false,receivedBytes:0,expectedBytes:0};
 if(cmd==='try_style_pack_text')return {packId:args.stylePackId,packName:'Writing',outputText:args.stylePackId.startsWith('builtin.')?args.rawText:'Hi Mia, please confirm the review.',usedTextService:true,elapsedMs:250};
 if(cmd==='try_local_translation_text')return {packId:'local_translation:en:zh',packName:'Local',outputText:'你好Mia，请确认评审安排。',usedTextService:false,elapsedMs:300};
 return mock();};
""")
  def fixture(route):
   response=route.fetch();body=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert body.count(old)==1
   route.fulfill(response=response,body=body.replace(old,'if (!isTauri) {\n    return window.__intentFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture);page.goto(a.url,wait_until='networkidle')
  def open_workspace():
   page.get_by_role('button',name='Open text workspace',exact=True).click();return page.locator('dialog.ol-style-trial[open]')
  def local_translation(d):
   d.get_by_role('button',name='Translate this result…',exact=True).click()
   d.get_by_role('combobox',name='Translate output to',exact=True).select_option('简体中文')
   d.get_by_role('combobox',name='Original language',exact=True).select_option('en')
   d.get_by_role('button',name='Translate',exact=True).click();expect(d.get_by_label('Result text',exact=True)).to_have_value(translated)
  d=open_workspace();expect(d.locator('[data-drafting-entry]')).to_be_visible()
  r['initialPublicPurposeHint']=d.locator('[data-drafting-entry] p').inner_text()
  d.locator('[data-drafting-entry] button').click();style=d.locator('select:has(option[value="builtin.light"])');expect(style).not_to_have_value('builtin.raw');draft_style=style.input_value()
  d.get_by_label('Your text',exact=True).fill(brief);d.get_by_role('button',name=re.compile(r'^(?:Try this text|Process text|Generate draft|Preparing…)$'),exact=True).click();expect(d.get_by_label('Result text',exact=True)).to_have_value(generated)
  d.get_by_label('Draft name',exact=True).fill('First drafted body');d.get_by_role('button',name='Save draft',exact=True).click();expect(d.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(1)
  local_translation(d);d.get_by_label('Result text',exact=True).fill(manual)
  before=page.evaluate('window.__intentCalls.length');d.get_by_role('button',name='Refine this translation…',exact=True).click()
  expect(d.locator('select:has(option[value="builtin.light"])')).to_have_value('builtin.light')
  expect(d.get_by_label('Your text',exact=True)).to_have_value(manual)
  expect(d.locator('[data-workspace-original] pre')).to_have_text(brief)
  expect(d.locator('[data-text-previous-step]')).to_contain_text(manual)
  calls=page.evaluate('window.__intentCalls')[before:];assert not any(x['cmd'].startswith('try_') or x['cmd'] in {'save_text_draft','set_active_style_pack'} for x in calls)
  r['checks'].append('Explicit translation refinement switches instruction drafting to Light, retains exact manual translated input/first brief/previous result and performs no request/save/style activation')
  for width,height in [(360,640),(1240,850)]:
   page.set_viewport_size({'width':width,'height':height})
   for action in [d.get_by_role('button',name=re.compile(r'^(?:Try this text|Process text|Generate draft|Preparing…)$'),exact=True),d.get_by_role('button',name='Close',exact=True).first]:
    assert action.evaluate('e=>{const r=e.getBoundingClientRect(),hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);return r.left>=0&&r.right<=innerWidth&&r.top>=0&&r.bottom<=innerHeight&&!!hit&&e.contains(hit)}')
  r['checks'].append('The newly visible draft-purpose hint does not hide Close or explicit Submit in narrow or wide refinement workspaces')
  d.get_by_role('button',name=re.compile(r'^(?:Try this text|Process text|Generate draft|Preparing…)$'),exact=True).click();expect(d.get_by_label('Result text',exact=True)).to_have_value(manual)
  writes=page.evaluate('window.__intentCalls.filter(x=>x.cmd==="try_style_pack_text").map(x=>x.args)');assert len(writes)==2 and writes[0]['stylePackId']==draft_style and writes[0]['rawText']==brief and writes[1]=={'stylePackId':'builtin.light','rawText':manual}
  d.get_by_label('Draft name',exact=True).fill('Reviewed translated body');d.get_by_role('button',name='Save draft',exact=True).click();expect(d.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(1)
  saves=page.evaluate('window.__intentCalls.filter(x=>x.cmd==="save_text_draft").map(x=>x.args)');assert len(saves)==2 and saves[0]['text']==generated and saves[1]['text']==manual and saves[0]['id']!=saves[1]['id'] and all(x['original']==brief for x in saves)
  r['checks'].append('Only explicit second submit sends the current translated edit to Light; separate v1 saves retain generated first body, manual final body and exact first instruction input')
  page.screenshot(path=str(a.report.with_suffix('.png')));d.get_by_role('button',name='Close',exact=True).first.click()
  d=open_workspace();d.get_by_label('Your text',exact=True).fill(generated);d.locator('select:has(option[value="builtin.formal"])').select_option('builtin.formal')
  d.get_by_role('button',name=re.compile(r'^(?:Try this text|Process text|Generate draft|Preparing…)$'),exact=True).click();expect(d.get_by_label('Result text',exact=True)).to_have_value(generated);local_translation(d)
  before=page.evaluate('window.__intentCalls.length');d.get_by_role('button',name='Refine this translation…',exact=True).click();expect(d.locator('select:has(option[value="builtin.formal"])')).to_have_value('builtin.formal')
  calls=page.evaluate('window.__intentCalls')[before:];assert not any(x['cmd'].startswith('try_') or x['cmd'] in {'save_text_draft','set_active_style_pack'} for x in calls)
  r['checks'].append('An existing user-selected Formal style remains selected across translation refinement; only the instruction-drafting task gets a new default')
  r['passed']=True
 except Exception as error:r['failure']=repr(error);page.screenshot(path=str(a.report.with_suffix('.png')))
 finally:b.close();a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
print(json.dumps({'passed':r['passed'],'checks':len(r['checks']),'failure':r.get('failure')}))
raise SystemExit(0 if r['passed'] else 1)
