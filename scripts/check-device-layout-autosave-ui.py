"""Actual React layout application while an earlier auto-save is pending.

Controlled IPC fixture in a headless browser; no real device, service, clipboard
or Owner desktop input. Reuses the existing auto-save fixture, not product mocks
masquerading as physical device evidence.
"""
import argparse, ast, datetime, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--expect',choices=['preserved','lost'],default='preserved');p.add_argument('--failure',choices=['none','earlier','later'],default='none');p.add_argument('--switch-device',action='store_true');a=p.parse_args()
repo=Path(__file__).resolve().parents[1]
fixture_tree=ast.parse((repo/'scripts/check-device-autosave-ui.py').read_text('utf8'))
init=next(n.args[0].value for n in ast.walk(fixture_tree) if isinstance(n,ast.Call) and isinstance(n.func,ast.Attribute) and n.func.attr=='add_init_script')
r={'startedAt':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':False}
a.report.parent.mkdir(parents=True,exist_ok=True)
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge');page=b.new_page(viewport={'width':1500,'height':1300});page.set_default_timeout(7000)
 try:
  page.add_init_script(init)
  if a.failure=='later':page.add_init_script('const originalFixture=window.__fixture;let keyWrites=0;window.__fixture=async(cmd,args,mock)=>{const result=await originalFixture(cmd,args,mock);if(cmd==="set_selected_device_key_profile"&&++keyWrites===1)window.__failNext=cmd;return result;};')
  def route_ipc(route):
   response=route.fetch();source=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert source.count(old)==1
   route.fulfill(response=response,body=source.replace(old,'if (!isTauri) {\n    return window.__fixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),route_ipc)
  page.goto(a.url,wait_until='networkidle');page.locator('button[title]').first.click();page.locator('.ol-settings-nav button').first.click()
  core=page.locator('.ol-device-settings-card');keys=page.locator('fieldset').filter(has=page.get_by_role('combobox',name='选择设备键动作').first)
  expect(core.locator('input[placeholder=listener]')).to_have_value('Alpha')
  library=keys.get_by_role('button',name='已保存的布局',exact=True);library.click();d=page.get_by_role('dialog').last
  d.get_by_label('布局名称').fill('Latest layout marker');d.get_by_label('动作',exact=True).select_option('pasteTemplate');d.get_by_label('要粘贴的文字').fill('Latest layout must survive')
  d.get_by_role('button',name='保存布局',exact=True).click();expect(d.get_by_role('status')).to_contain_text('已保存在本机');d.get_by_role('button',name='关闭',exact=True).click()
  k=keys.get_by_role('combobox',name='选择设备键动作').nth(11);k.scroll_into_view_if_needed();k.focus();page.mouse.move(5,5);k.press('ArrowDown')
  opts=page.get_by_role('listbox').get_by_role('option');values=opts.evaluate_all('(es)=>es.map(e=>({text:e.innerText.trim(),selected:e.getAttribute("aria-selected")==="true"}))')
  current=next(i for i,x in enumerate(values) if x['selected']);target=next(i for i,x in enumerate(values) if x['text']=='复制 Ctrl+C')
  for _ in range((target-current)%len(values)):k.press('ArrowDown')
  page.evaluate('window.__holdRead="usb:A"')
  if a.failure=='earlier':page.evaluate('window.__failNext="set_selected_device_key_profile"')
  k.press('Enter');library.click()
  page.wait_for_function('window.__held.length===1');r['libraryWasOpenedBeforeEarlierAutosaveCompleted']=True
  d=page.get_by_role('dialog').last;d.get_by_label('已保存的布局').select_option(label='Latest layout marker');d.get_by_role('button',name='应用到当前设备',exact=True).click()
  page.wait_for_timeout(250);r['layoutAppliedWhileEarlierReadWasPending']=page.evaluate('window.__held.length===1')
  if a.switch_device:
   target=core.get_by_role('combobox',name='设置设备');target.scroll_into_view_if_needed();target.focus();page.mouse.move(5,5);target.press('ArrowDown')
   opts=page.get_by_role('listbox').get_by_role('option');values=opts.evaluate_all('(es)=>es.map(e=>({text:e.innerText.trim(),selected:e.getAttribute("aria-selected")==="true"}))');current=next(i for i,x in enumerate(values) if x['selected']);dest=next(i for i,x in enumerate(values) if x['text']=='USB B')
   for _ in range((dest-current)%len(values)):target.press('ArrowDown')
   target.press('Enter');expect(core.locator('input[placeholder=listener]')).to_have_value('Bravo')
  page.evaluate('window.__holdRead=null;window.__held.splice(0).forEach(r=>r())')
  if a.failure!='none':
   expect(keys).to_contain_text('保存失败，修改已保留');expect(keys.get_by_role('combobox',name='选择设备键动作').first).to_contain_text('输入固定文字');r['failedSaveRetainedLatestLayoutInUi']=True
   keys.get_by_role('button',name='重试保存',exact=True).click();expect(keys).to_contain_text('已保存到设备')
  page.wait_for_function('window.__calls.some(x=>x.cmd==="set_selected_device_key_profile")');page.wait_for_timeout(500)
  r['actualProfile']=page.evaluate('window.__boards.A.profile');r['latestLayoutPreserved']=r['actualProfile']['deviceCustomKeys']['key1']['pasteTemplate']=='Latest layout must survive'
  r['writes']=page.evaluate('window.__calls.filter(x=>x.cmd==="set_selected_device_key_profile")');r['visibleStatuses']=page.get_by_role('status').all_text_contents();r['passed']=r['latestLayoutPreserved']==(a.expect=='preserved')
  if a.switch_device:r['switchKeptWritesOnOriginalBoard']=all(x['args']['device']['id']=='usb:A' for x in r['writes']) and page.evaluate('window.__boards.B.profile.deviceCustomKeys.key1.action')=='disabled';r['passed']=r['passed'] and r['switchKeptWritesOnOriginalBoard']
 except Exception as e:r['error']=str(e)
 finally:
  page.screenshot(path=str(a.report.with_suffix('.png')),full_page=True);b.close()
r['finishedAt']=datetime.datetime.now().astimezone().isoformat();a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n','utf8');print(json.dumps({k:v for k,v in r.items() if k not in ['writes','actualProfile']},ensure_ascii=False));raise SystemExit(0 if r['passed'] else 1)
