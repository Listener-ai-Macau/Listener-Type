"""Photo-linked layout selection and independent software practice press mappings."""
import argparse,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',required=True,type=Path);a=p.parse_args();checks=[]
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=b.new_page(viewport={'width':1280,'height':1000});page.set_default_timeout(6000)
  page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');window.__layoutCalls=[];window.__layoutFixture=(cmd,args,mock)=>{window.__layoutCalls.push({cmd,args});return mock();};Object.defineProperty(navigator,'clipboard',{value:{readText:()=>{throw Error('No clipboard read expected')},writeText:()=>{throw Error('No clipboard write expected')}}});""")
  def fixture(route):
   r=route.fetch();s=r.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1;route.fulfill(response=r,body=s.replace(old,'if (!isTauri) {\n    return window.__layoutFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture);page.goto(a.url,wait_until='networkidle')
  entry=page.locator('.ol-listener-hardware-entry');expect(entry).to_contain_text('US$49');expect(entry).to_contain_text('Type is free');expect(entry).to_contain_text('cloud usage is extra');assert entry.locator('img').evaluate('(e)=>e.complete&&e.naturalWidth===1400')
  entry.get_by_role('button',name='Design my key layout',exact=True).click();d=page.get_by_role('dialog',name='Your key layouts',exact=True);m=d.get_by_role('region',name='Your layout on Listener',exact=True)
  expect(m).to_contain_text('not a live connection or device write');assert m.locator('.ol-device-layout-photo span[data-selected=true]').inner_text()=='1'
  m.get_by_role('button',name=re.compile('^Key 4 ')).click();assert m.locator('.ol-device-layout-photo span[data-selected=true]').inner_text()=='4';expect(d.locator('.ol-device-layout-editor h3')).to_have_text('Key 4');checks.append('new optional entry shows actual photograph, price and service boundary; selecting a key highlights its actual position without running it')
  m.get_by_role('button',name=re.compile('^Key 1 ')).click();d.get_by_label('Layout name',exact=True).fill('My three presses')
  values=[('deviceCustomKeys','Single — 😀'),('deviceCustomKeyDoubleClicks','Double — 返信'),('deviceCustomKeyLongPresses','Hold — 保留\n条件')]
  for group,text in values:
   d.get_by_label('Press',exact=True).select_option(group);d.get_by_label('Action',exact=True).select_option('pasteTemplate');d.get_by_label('Text to paste',exact=True).fill(text)
  d.get_by_role('button',name='Save layout',exact=True).click();expect(d.get_by_role('status')).to_contain_text('Saved on this computer')
  d.get_by_role('button',name='Practice here',exact=True).click();area=d.get_by_role('region',name='Try your four keys',exact=True);field=area.get_by_label('Practice text',exact=True)
  for group,text in values:
   field.fill('');area.get_by_label('Press to practice',exact=True).select_option(group);k=area.get_by_role('button',name=re.compile('^Key 1 '));expect(k.locator('.ol-device-key-phrase')).to_have_text(text);k.click();expect(field).to_have_value(text)
  checks.append('single, double and hold each use their own saved Unicode/newline phrase; switching press does not mix mappings')
  area.get_by_label('Press to practice',exact=True).select_option('deviceCustomKeys');field.fill('Keep my draft');field.press('Control+A');area.get_by_role('button',name=re.compile('^Key 1 ')).click();expect(field).to_have_value(values[0][1]);checks.append('ordinary focus and text selection survive clicking the press selector and practice key')
  d.get_by_role('button',name='Close',exact=True).click();entry.get_by_role('button',name='Design my key layout',exact=True).click();d.get_by_role('button',name='Practice here',exact=True).click();expect(field).to_have_value('')
  for group,text in values:
   area.get_by_label('Press to practice',exact=True).select_option(group);expect(area.get_by_role('button',name=re.compile('^Key 1 ')).locator('.ol-device-key-phrase')).to_have_text(text)
  checks.append('reopening saved layout retains all three mappings while temporary practice text is cleared')
  d.get_by_role('button',name='Configure & save',exact=True).click();m.get_by_role('button',name=re.compile('^Knob ')).click();assert m.locator('.ol-device-layout-photo span[data-selected=true]').inner_text()=='●';assert d.get_by_label('Press',exact=True).count()==0
  checks.append('knob preview uses its fixed single-click mapping; editable double/hold remain four-key actions only')
  for width,height in [(1280,1000),(500,700)]:
   page.set_viewport_size({'width':width,'height':height});m.scroll_into_view_if_needed();assert d.evaluate('(e)=>e.scrollWidth<=e.clientWidth+1');assert m.locator('img').evaluate('(e)=>e.complete&&e.naturalWidth>0');d.screenshot(path=str(a.report.with_name(f'hardware-layout-{width}.png')))
  calls=page.evaluate('window.__layoutCalls');assert not any(x['cmd'] in {'set_selected_device_key_profile','set_selected_device_settings','update_preferences','try_style_pack_text','start_dictation','set_hotkey','copy_style_trial_result','insert_style_trial_result'} for x in calls);checks.append('photo/press practice stays local and fits wide/narrow windows; no device, settings, cloud, clipboard or recording side effect')
  a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps({'passed':True,'scope':'Headless actual frontend with local store fixture, not physical or customer-purchase qualification.','checks':checks},ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps({'passed':True,'checks':checks},ensure_ascii=False))
 finally:b.close()
