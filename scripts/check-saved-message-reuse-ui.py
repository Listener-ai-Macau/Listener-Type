"""Actual React saved-message labels and explicit local key-phrase practice.
Owned IPC/list fixtures only; no real device, clipboard, services or storage writes.
"""
import argparse,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect

p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
base='Could you please send me the final version of the picture?'
deadline='Could you please send me the final version of the picture by 5 pm today?'
rows=[{'id':id,'createdAt':'2026-10-24T08:00:00Z','name':'Local translation','original':'把最终版的图片发给我。','text':text}
      for id,text in [('base',base),('deadline',deadline),('long','A'*2001)]]
checks=[]
with sync_playwright() as pw:
 b=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=b.new_page(viewport={'width':1280,'height':1000});page.set_default_timeout(7000)
  page.add_init_script("""localStorage.setItem('ol.locale','en');window.__reuseCalls=[];window.__reuseClipboard=[];window.__reusePhase='startup';
  Object.defineProperty(navigator,'clipboard',{value:{writeText:t=>{window.__reuseClipboard.push(t);return Promise.resolve();}}});
  window.__reuseFixture=(cmd,args,mock)=>{
   window.__reuseCalls.push({cmd,args,phase:window.__reusePhase,stack:cmd==='set_settings'?new Error().stack:null});
   if(cmd==='list_saved_text_drafts')return ROWS;
   if(cmd==='list_local_device_layouts')return [];
   return mock();
  };""".replace('ROWS',json.dumps(rows,ensure_ascii=False)))
  def fixture(route):
   response=route.fetch();s=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1
   route.fulfill(response=response,body=s.replace(old,'if (!isTauri) {\n    return window.__reuseFixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  page.goto(a.url,wait_until='networkidle');page.evaluate("window.__reusePhase='library'");page.locator('aside nav').get_by_role('button',name='History',exact=True).click()
  page.get_by_role('button',name='Saved drafts',exact=True).first.click();saved=page.get_by_role('dialog',name='Saved drafts',exact=True)
  chooser=saved.get_by_role('combobox',name='Choose a saved draft',exact=True);expect(chooser.locator('option')).to_have_count(3)
  old_label=chooser.locator('option[value="base"]').inner_text();new_label=chooser.locator('option[value="deadline"]').inner_text()
  assert old_label!=new_label and 'by 5 pm today?' in new_label and 'by 5 pm today?' not in old_label
  assert old_label.startswith('Could you please send me') and new_label.startswith('Could you please send me')
  checks.append('Actual170 same-opening variants remain distinguishable by their ending even with identical name/date; bounded labels retain their beginning')
  chooser.select_option('deadline');expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_value(deadline)
  page.evaluate("window.__reusePhase='phrase'");saved.get_by_role('button',name='Use as a key phrase…',exact=True).click()
  layout=page.locator('dialog[open]').filter(has=page.locator('#device-layout-title'))
  expect(layout.get_by_role('textbox',name='Phrase to use',exact=True)).to_have_value(deadline)
  expect(layout).to_contain_text('Your saved message stays unchanged; this does not write to a device.')
  expect(layout).not_to_contain_text('Your original result stays in the trial window.')
  checks.append('Explicit phrase entry freezes the selected complete saved message and explains saved-library/local-layout ownership')
  layout.get_by_role('button',name=re.compile(r'^Use for Key 1')).click()
  layout.get_by_role('button',name='Practice here',exact=True).click();practice=layout.get_by_role('region',name='Try your four keys',exact=True)
  practice.locator('.ol-device-practice-keys > button').first.click()
  expect(practice.get_by_role('textbox',name='Practice text',exact=True)).to_have_value(deadline)
  page.screenshot(path=str(a.report.with_name(a.report.stem+'-practice.png')))
  checks.append('After choosing the phrase for a local draft key, clicking its software practice key inserts that exact saved message into the scoped practice field')
  layout.get_by_role('button',name='Close',exact=True).click();expect(layout.get_by_role('alert')).to_be_visible()
  layout.get_by_role('button',name='Continue',exact=True).click();expect(layout).to_have_count(0);expect(saved).to_be_visible()
  expect(chooser).to_have_value('deadline');expect(saved.get_by_role('textbox',name='Saved text',exact=True)).to_have_value(deadline)
  checks.append('Discarding the unsaved local layout returns to the same saved record without closing or mutating its library')
  chooser.select_option('long');expect(saved.get_by_role('button',name='Use as a key phrase…',exact=True)).to_be_disabled();expect(saved).to_contain_text('2,000 characters or fewer')
  checks.append('Messages over the existing key-phrase limit are retained and explicitly blocked, never silently truncated')
  calls=page.evaluate('window.__reuseCalls');a.report.with_name(a.report.stem+'-calls.json').write_text(json.dumps(calls,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
  forbidden=[x['cmd'] for x in calls if x['phase']=='phrase' and re.match(r'^(set_|update_|save_|delete_|start_|copy_|insert_|try_)',x['cmd'])]
  assert not forbidden,forbidden;assert page.evaluate('window.__reuseClipboard')==[]
  checks.append('Opening, local binding practice and discard make no recording, text-service, clipboard, device, preferences, layout-save or draft-write request')
 finally:b.close()
result={'passed':True,'scope':__doc__,'checks':checks,'labels':{'base':old_label,'deadline':new_label},'calls':[x['cmd'] for x in calls]}
a.report.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf8');print(json.dumps({'passed':True,'checks':checks},ensure_ascii=False))
