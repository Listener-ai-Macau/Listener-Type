"""Owned, headless UI check of local layout drafts; no native device or cloud use."""
import argparse,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',required=True,type=Path);a=p.parse_args();checks=[]
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=b.new_page(viewport={'width':1280,'height':1000});page.set_default_timeout(6000)
  page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');window.__layoutCalls=[];window.__layoutSaveError='';window.__layoutListError=false;window.__deviceProfile=null;window.__layoutFixture=(cmd,args,mock)=>{window.__layoutCalls.push({cmd,args});if(cmd==='list_local_device_layouts'&&window.__layoutListError)throw new Error('device_layout_invalid_file');if(cmd==='save_local_device_layout'&&window.__layoutSaveError)throw new Error(window.__layoutSaveError);if(cmd==='list_settings_devices')return [{id:'usb:layout-a',transport:'usb',label:'Layout board A'},{id:'usb:layout-b',transport:'usb',label:'Layout board B'}];if(cmd==='get_selected_device_settings'&&window.__deviceProfile)return {...mock(),revision:7,readToken:'fresh:'+args.device.id,keys:{deviceUid:args.device.id,revision:3,profile:window.__deviceProfile}};return mock();};""")
  def fixture(route):
   response=route.fetch();s=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1
   route.fulfill(response=response,body=s.replace(old,'if (!isTauri) {\n    return window.__layoutFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture);page.goto(a.url,wait_until='networkidle')
  page.get_by_role('button',name='Design your keys',exact=True).click();d=page.get_by_role('dialog')
  expect(d).to_be_visible();expect(d.get_by_label('Layout name')).to_have_value('');expect(d.get_by_role('button',name='Save layout',exact=True)).to_be_disabled()
  d.get_by_label('Layout name').fill('Support replies');d.get_by_label('Action',exact=True).select_option('pasteTemplate');d.get_by_label('Text to paste').fill('Thanks for the report. Could you send the order number?')
  d.get_by_role('button',name='Close',exact=True).click();expect(d.get_by_role('alert')).to_contain_text('unsaved changes');d.get_by_role('button',name='Keep editing',exact=True).click();expect(d.get_by_label('Text to paste')).to_have_value('Thanks for the report. Could you send the order number?');checks.append('unsaved close asks without losing text')
  page.evaluate("window.__layoutSaveError='device_layout_stale'");d.get_by_role('button',name='Save layout',exact=True).click();expect(d.get_by_role('status')).to_contain_text('changed elsewhere');expect(d.get_by_label('Layout name')).to_have_value('Support replies');checks.append('failed save preserves draft and explains stale revision')
  page.evaluate("window.__layoutSaveError=''");d.get_by_role('button',name='Save layout',exact=True).click();expect(d.get_by_role('status')).to_contain_text('No device was changed');d.get_by_role('button',name='Close',exact=True).click()
  page.get_by_role('button',name='Design your keys',exact=True).click();d.get_by_label('Saved layouts').select_option(label='Support replies');expect(d.get_by_label('Text to paste')).to_have_value('Thanks for the report. Could you send the order number?');checks.append('saved phrase layout reopens with its name and body')
  first=page.evaluate("window.__layoutCalls.filter(x=>x.cmd==='save_local_device_layout').at(-1).args");assert first['profile']['deviceCustomKeys']['key2']['shortcut']=={'primary':'C','modifiers':['ctrl']}
  assert not d.get_by_role('button',name='Load into selected device draft').count();checks.append('Overview local library cannot write or load into an unspecified device')
  d.get_by_role('button',name='New layout',exact=True).click();d.get_by_label('Layout name').fill('Review');d.get_by_label('Shortcut key').select_option('K');d.get_by_label('ctrl',exact=True).check();d.get_by_role('button',name='Save layout',exact=True).click();expect(d.get_by_role('status')).to_contain_text('Saved on this computer')
  d.get_by_label('Saved layouts').select_option(label='Support replies');expect(d.get_by_label('Text to paste')).to_contain_text('order number');d.get_by_label('Saved layouts').select_option(label='Review');expect(d.get_by_label('Shortcut key')).to_have_value('K');expect(d.get_by_label('ctrl',exact=True)).to_be_checked();checks.append('two independent saved layouts keep separate text and shortcuts')
  d.get_by_role('button',name='Delete layout',exact=True).click();expect(d.get_by_role('alert')).to_contain_text('Delete “Review”');d.get_by_role('button',name='Continue',exact=True).click();expect(d.get_by_label('Saved layouts').locator('option')).to_have_count(2);checks.append('explicit deletion leaves the other layout')
  d.get_by_role('button',name='Close',exact=True).click();page.evaluate('window.__layoutListError=true');page.get_by_role('button',name='Design your keys',exact=True).click();expect(d.get_by_role('alert')).to_contain_text('kept unchanged');d.get_by_label('Layout name').fill('Do not overwrite');expect(d.get_by_role('button',name='Save layout',exact=True)).to_be_disabled();checks.append('unreadable library blocks overwrite while keeping editor available')
  page.evaluate('window.__layoutListError=false');d.get_by_role('button',name='Retry',exact=True).click();expect(d.get_by_role('alert')).to_have_count(0)
  d.get_by_role('button',name='Close',exact=True).click();d.get_by_role('button',name='Continue',exact=True).click();page.get_by_role('button',name='Design your keys',exact=True).click();d.get_by_label('Saved layouts').select_option(label='Support replies')
  for width,height in [(1280,1000),(760,700)]:
   page.set_viewport_size({'width':width,'height':height});box=d.bounding_box();assert box['x']>=0 and box['x']+box['width']<=width+1 and box['y']>=0 and box['y']+box['height']<=height+1
   assert d.evaluate('(e)=>e.scrollWidth<=e.clientWidth+1');d.screenshot(path=str(a.report.with_name('device-layouts-'+str(width)+'.png')))
  checks.append('wide and narrow dialog fits its viewport without horizontal overflow')
  calls=page.evaluate('window.__layoutCalls');forbidden=['set_selected_device_key_profile','set_selected_device_settings','update_preferences','try_style_pack_text','start_dictation','set_hotkey','set_qa_hotkey']
  assert not any(x['cmd'] in forbidden for x in calls);checks.append('local design/save/reload/delete sends no hardware, preference, hotkey or cloud mutation')
  d.get_by_role('button',name='Close',exact=True).click();page.set_viewport_size({'width':1280,'height':1000})
  page.evaluate("window.__deviceProfile=window.__layoutCalls.find(x=>x.cmd==='save_local_device_layout').args.profile")
  page.get_by_role('button',name='Settings',exact=True).click();page.get_by_role('button',name='Saved layouts',exact=True).click()
  d.get_by_label('Saved layouts').select_option(label='Support replies');before=page.evaluate("window.__layoutCalls.filter(x=>x.cmd==='set_selected_device_key_profile').length")
  d.get_by_role('button',name='Load into selected device draft',exact=True).click();expect(page.get_by_role('status').filter(has_text='Layout loaded')).to_be_visible();assert page.evaluate("window.__layoutCalls.filter(x=>x.cmd==='set_selected_device_key_profile').length")==before
  checks.append('loading a saved layout into selected board only prepares a draft')
  page.get_by_role('button',name='Write',exact=True).last.click();expect(page.get_by_role('status').filter(has_text='Sent to device')).to_be_visible()
  write=page.evaluate("window.__layoutCalls.findLast(x=>x.cmd==='set_selected_device_key_profile')");assert write['args']['device']['id']=='usb:layout-a' and write['args']['revision']==7 and write['args']['readToken']=='fresh:usb:layout-a' and write['args']['profileRevision']==3
  assert write['args']['profile']['deviceCustomKeys']['key1']['pasteTemplate'].endswith('order number?');checks.append('explicit write uses the current exact board and freshly read setting/profile revisions')
  page.get_by_role('button',name='Saved layouts',exact=True).click();d.get_by_label('Saved layouts').select_option(label='Support replies');d.get_by_role('button',name='Load into selected device draft',exact=True).click()
  target=page.get_by_role('combobox',name='Device to configure',exact=True);target.scroll_into_view_if_needed();target.focus();target.press('ArrowDown');target.press('ArrowDown');target.press('Enter');expect(target).to_contain_text('Layout board B');expect(page.get_by_role('button',name='Write',exact=True).last).to_be_disabled()
  assert page.evaluate("window.__layoutCalls.filter(x=>x.cmd==='set_selected_device_key_profile').length")==1;checks.append('switching to another board clears the prior board draft without writing it')
  a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps({'scope':'Isolated headless frontend with local IPC fixture; not native persistence, real device write or customer benefit.','passed':True,'checks':checks,'commands':[x['cmd'] for x in page.evaluate('window.__layoutCalls')]},ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps({'passed':True,'checks':checks},ensure_ascii=False))
 finally:b.close()
