"""Actual React cloud-translation fact boundary, immutable saves and recovery.

Controlled IPC replies, isolated browser/mock store, no live service/desktop,
microphone, external clipboard or hardware qualification.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True)
a=p.parse_args();a.report.parent.mkdir(parents=True,exist_ok=True)
source='支付页仍有一个手机端按钮错位，预计明天下午三点前修好。请大家先不要发布，等我确认后再合并。另外，周五客户演示仍是十点，不是十一点。会议控制在四十五分钟。'
reply="The payment page still has one misaligned button on mobile, expected to be fixed by 3 PM tomorrow. Please do not publish yet; wait for my confirmation before merging. Friday's client demo is still at 10 AM, not 11 AM. Keep the meeting to 45 minutes."
corrected=reply.replace('10 AM','10').replace('11 AM','11')
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':False,'checks':[]}
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu'])
 page=browser.new_page(viewport={'width':1240,'height':850})
 try:
  page.add_init_script("""
localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');window.__factCalls=[];
window.__factFixture=(cmd,args,mock)=>{window.__factCalls.push({cmd,args});
 if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:true,textServiceConfigured:true};
 if(cmd==='try_translation_text')return {packId:'translation:'+args.targetLanguage,packName:args.targetLanguage,outputText:window.__factReply,usedTextService:true,elapsedMs:1200};
 if(cmd==='try_style_pack_text')return {packId:args.stylePackId,packName:'Light',outputText:args.rawText,usedTextService:true,elapsedMs:1200};
 return mock();};
""")
  def fixture(route):
   response=route.fetch();body=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert body.count(old)==1
   route.fulfill(response=response,body=body.replace(old,'if (!isTauri) {\n    return window.__factFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  page.goto(a.url,wait_until='networkidle');page.evaluate('(s)=>window.__factReply=s',reply)
  page.get_by_role('button',name='Open text workspace',exact=True).click();d=page.locator('dialog.ol-style-trial[open]')
  d.get_by_label('Your text',exact=True).fill(source)
  d.get_by_role('combobox',name='What would you like to do?',exact=True).select_option('translate')
  d.get_by_role('combobox',name='Translate output to',exact=True).select_option('English')
  d.get_by_role('combobox',name='Translation method',exact=True).select_option('cloud')
  d.get_by_role('button',name='Translate',exact=True).click()
  editor=d.get_by_label('Result text',exact=True);expect(editor).to_have_value(corrected)
  r['checks'].append('Actual cloud result removes only unsupported10AM/11AM, retains explicit3PM deadline,45minutes,negation and valid one-button translation')
  d.get_by_label('Draft name',exact=True).fill('Source-backed cloud translation')
  d.get_by_role('button',name='Save draft',exact=True).click();expect(d.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(1)
  saves=page.evaluate('window.__factCalls.filter(x=>x.cmd==="save_text_draft").map(x=>x.args)')
  assert len(saves)==1 and saves[0]['text']==corrected and saves[0]['original']==source
  manual=corrected.replace('at 10,','at 10 AM,')+'\nMy explicit manual revision.'
  editor.fill(manual);d.get_by_role('button',name='Save draft',exact=True).click();expect(d.get_by_role('button',name='Open this saved draft',exact=True)).to_have_count(1)
  saves=page.evaluate('window.__factCalls.filter(x=>x.cmd==="save_text_draft").map(x=>x.args)')
  assert len(saves)==2 and saves[0]['text']==corrected and saves[1]['text']==manual and all(s['original']==source for s in saves)
  assert len(page.evaluate('window.__factCalls.filter(x=>x.cmd==="try_translation_text")'))==1
  r['checks'].append('Explicit saves preserve original/corrected text and later manual AM as separate records, without reprocessing edits or another service call')
  changed=source.replace('三点','四点').replace('四十五分钟','三十分钟')
  d.get_by_label('Your text',exact=True).fill(changed);d.get_by_role('button',name='Translate',exact=True).click()
  expect(d.get_by_role('alert')).to_contain_text('could not be matched safely')
  expect(d.get_by_label('Your text',exact=True)).to_have_value(changed)
  expect(d.get_by_label('Result text',exact=True)).to_have_count(0)
  assert len(page.evaluate('window.__factCalls.filter(x=>x.cmd==="save_text_draft")'))==2
  assert len(page.evaluate('window.__factCalls.filter(x=>x.cmd==="try_translation_text")'))==2
  r['checks'].append('A stale time/duration response is not displayed/cached/saved; exact input remains and no automatic paid retry occurs')
  d.get_by_role('combobox',name='What would you like to do?',exact=True).select_option('text')
  d.get_by_label('Your text',exact=True).fill('我的消息保持原样。')
  d.locator('select:has(option[value="builtin.light"])').select_option('builtin.light')
  d.get_by_role('button',name=re.compile(r'^(?:Try this text|Process text|Generate draft|Preparing…)$'),exact=True).click()
  expect(d.get_by_label('Result text',exact=True)).to_have_value('我的消息保持原样。')
  expect(d).not_to_contain_text('{{seconds}}')
  expect(d.locator('p[role="note"]').filter(has_text=re.compile('^The text service returned',re.I))).to_contain_text('1.2')
  calls=page.evaluate('window.__factCalls')
  assert sum(c['cmd']=='try_style_pack_text' for c in calls)==1
  assert not any(c['cmd'] in {'try_local_translation_text','insert_style_trial_result','start_dictation'} for c in calls)
  r['checks'].append('Unchanged service outcome remains honestly disclosed with real elapsed seconds; no fabricated rewrite or hidden retranslation/insertion/microphone request')
  page.screenshot(path=str(a.report.with_suffix('.png')));r['passed']=True
 except Exception as error:
  r['failure']=repr(error);page.screenshot(path=str(a.report.with_suffix('.png')))
 finally:
  browser.close();a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
print(json.dumps({'passed':r['passed'],'checks':len(r['checks']),'failure':r.get('failure')}))
raise SystemExit(0 if r['passed'] else 1)
