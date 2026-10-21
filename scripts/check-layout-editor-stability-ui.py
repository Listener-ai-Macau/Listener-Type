"""Reproduce slow-library load/name movement and compact key editing without device I/O."""
import argparse,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',required=True,type=Path);p.add_argument('--baseline',action='store_true');a=p.parse_args();checks=[]
a.report.parent.mkdir(parents=True,exist_ok=True)
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=b.new_page(viewport={'width':1280,'height':1000});page.set_default_timeout(6000)
  page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');window.__calls=[];window.__layoutFixture=(cmd,args,mock)=>{window.__calls.push({cmd,args});if(cmd==='list_local_device_layouts')return new Promise(resolve=>window.__releaseLayouts=()=>resolve(mock()));return mock();};""")
  def fixture(route):
   r=route.fetch();s=r.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1;route.fulfill(response=r,body=s.replace(old,'if (!isTauri) {\n    return window.__layoutFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture);page.goto(a.url,wait_until='networkidle')
  page.get_by_role('button',name='Design my key layout',exact=True).click();d=page.get_by_role('dialog',name='Your key layouts',exact=True);field=d.get_by_label('Layout name',exact=True)
  expect(d.get_by_role('status')).to_have_text('Loading…');before=field.bounding_box();field.fill('A draft begun during loading')
  page.evaluate('window.__releaseLayouts()');expect(d.get_by_role('status')).to_have_count(0);after=field.bounding_box();movement=after['y']-before['y'];expect(field).to_have_value('A draft begun during loading')
  if not a.baseline:assert abs(movement)<1,(before,after)
  checks.append({'slowLoadNameMovementPx':movement,'typedDraftRetained':True})
  if not a.baseline:
   editor=d.locator('.ol-device-layout-editor');photo=d.locator('.ol-device-layout-map figure');pad=d.locator('.ol-device-layout-map .ol-device-layout-pad');e=editor.bounding_box();f=photo.bounding_box();k=pad.bounding_box()
   assert abs(e['y']-f['y'])<2 and e['x']+e['width']<=f['x']+1 and k['y']+k['height']<=e['y']+1,(e,f,k)
   checks.append('wide view selects a key once above its editor, with actual photo beside rather than above the form')
   d.get_by_label('Action',exact=True).select_option('pasteTemplate');phrase='Please confirm Friday — 返信😀\nKeep this condition.';d.get_by_label('Text to paste',exact=True).fill(phrase)
   d.get_by_role('button',name='Save layout',exact=True).click();expect(d.get_by_role('status')).to_contain_text('Saved on this computer')
   saved=page.evaluate("window.__calls.findLast(x=>x.cmd==='save_local_device_layout').args");assert saved['name']=='A draft begun during loading' and saved['profile']['deviceCustomKeys']['key1']['pasteTemplate']==phrase
   checks.append('ordinary template editing and local save keep the name entered during load and exact Unicode/newline text')
   for width,height in [(1280,1000),(760,800),(500,700)]:
    page.set_viewport_size({'width':width,'height':height});assert d.evaluate('(e)=>e.scrollWidth<=e.clientWidth+1');editor.scroll_into_view_if_needed();assert editor.evaluate('(e)=>e.scrollWidth<=e.clientWidth+1');d.screenshot(path=str(a.report.with_name(f'layout-editor-{width}.png')))
   checks.append('editor and narrow dialog fit without horizontal overflow')
   assert not any(x['cmd'] in {'set_selected_device_key_profile','set_selected_device_settings','update_preferences','try_style_pack_text','start_dictation','copy_style_trial_result'} for x in page.evaluate('window.__calls'))
   checks.append('layout edit stays local; no device, cloud, daily preference, recording or clipboard mutation')
  a.report.write_text(json.dumps({'passed':True,'baseline':a.baseline,'scope':'Headless actual frontend with intentionally delayed local-library read; not native/customer/physical proof.','checks':checks},ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(a.report.read_text(encoding='utf-8'))
 finally:b.close()
