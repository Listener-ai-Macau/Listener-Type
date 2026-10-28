"""Actual buffered Formal result against the recorded220 source-fact counterexample.

Headless actual React, deterministic IPC/service reply and mock draft storage.
No live provider/model, extra service request, desktop, clipboard or hardware.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args();a.report.parent.mkdir(parents=True,exist_ok=True)
source='嗨,萨姆,\n\n星期四三点见吧 午餐前我会把笔记寄出去 请将预算维持在50美元,如果需要改变时间,请告诉我.'
reply='Hi Sam,\n\n周四下午三点见。午餐前我会把笔记寄出。请将预算控制在 50 美元以内。如需调整时间，请告知我。'
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'source':source,'serviceReply':reply,'passed':False,'checks':[]}
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu']);page=b.new_page(viewport={'width':1240,'height':800})
 try:
  page.add_init_script("""
localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');window.__factCalls=[];
window.__factFixture=(cmd,args,mock)=>{window.__factCalls.push({cmd,args});
 if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:true,textServiceConfigured:true};
 if(cmd==='try_style_pack_text')return {packId:args.stylePackId,packName:'Formal',outputText:window.__factReply,usedTextService:true,elapsedMs:900};
 return mock();};
""")
  def fixture(route):
   response=route.fetch();body=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert body.count(old)==1
   route.fulfill(response=response,body=body.replace(old,'if (!isTauri) {\n    return window.__factFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  page.goto(a.url,wait_until='networkidle');page.evaluate('(s)=>window.__factReply=s',reply)
  page.get_by_role('button',name='Open text workspace',exact=True).click();d=page.locator('dialog.ol-style-trial[open]')
  d.get_by_label('Your text',exact=True).fill(source)
  d.locator('select:has(option[value="builtin.formal"])').select_option('builtin.formal')
  d.get_by_role('button',name='Try this text',exact=True).click()
  editor=d.get_by_label('Result text',exact=True);expect(editor).not_to_have_value('')
  actual=editor.input_value();r['actual']=actual
  r['noInventedDaypart']='下午' not in actual
  r['noInventedAmountBound']='以内' not in actual
  r['quantityRetained']=bool(re.search(r'50\s*美元',actual))
  page.screenshot(path=str(a.report.with_suffix('.png')))
  if r['noInventedDaypart'] and r['noInventedAmountBound'] and r['quantityRetained']:
   r['checks'].append('A source-unqualified clock and amount remain unqualified after one actual buffered Formal service reply')
   d.get_by_label('Draft name',exact=True).fill('Source-backed Formal result')
   d.get_by_role('button',name='Save draft',exact=True).click();expect(d.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(1)
   save=page.evaluate('window.__factCalls.filter(x=>x.cmd==="save_text_draft").at(-1).args')
   assert save['original']==source and save['text']==actual
   r['checks'].append('Explicit save receives the exact current original and source-corrected result; original input itself is never rewritten')
   manual=actual.replace('三点','下午三点')+'\nThis is my explicit manual revision.'
   editor.fill(manual);expect(editor).to_have_value(manual)
   d.get_by_role('button',name='Save draft',exact=True).click();expect(d.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(1)
   saves=page.evaluate('window.__factCalls.filter(x=>x.cmd==="save_text_draft").map(x=>x.args)')
   assert len(saves)==2 and saves[0]['text']==actual and saves[1]['text']==manual and all(x['original']==source for x in saves)
   r['checks'].append('A later explicit manual daypart edit is saved exactly as a separate draft; automatic validation never rewrites manual edits or the first saved version')
   calls=page.evaluate('window.__factCalls')
   assert sum(x['cmd']=='try_style_pack_text' for x in calls)==1
   assert not any(x['cmd'] in {'try_translation_text','try_local_translation_text','insert_style_trial_result','start_dictation'} for x in calls)
   r['checks'].append('No hidden source retranslation, paid validation/retry, microphone or target insertion request is started')
   changed=source.replace('三点','四点').replace('50美元','60美元')
   d.get_by_label('Your text',exact=True).fill(changed)
   d.get_by_role('button',name='Try this text',exact=True).click()
   expect(d.get_by_role('alert')).to_contain_text('could not be matched safely')
   expect(d.get_by_label('Your text',exact=True)).to_have_value(changed)
   expect(d.get_by_label('Result text',exact=True)).to_have_count(0)
   assert len(page.evaluate('window.__factCalls.filter(x=>x.cmd==="save_text_draft")'))==2
   assert len(page.evaluate('window.__factCalls.filter(x=>x.cmd==="try_style_pack_text")'))==2
   r['checks'].append('A response with different clock/amount values is not adopted, cached or saved; the exact changed input remains editable with no automatic paid retry')
   r['passed']=True
 except Exception as error:
  r['failure']=repr(error);page.screenshot(path=str(a.report.with_suffix('.png')))
 finally:
  b.close();a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
print(json.dumps({'passed':r['passed'],'noInventedDaypart':r.get('noInventedDaypart'),'noInventedAmountBound':r.get('noInventedAmountBound'),'failure':r.get('failure')}))
raise SystemExit(0 if r['passed'] else 1)
