"""History selection/copy evidence with controlled, local browser records only."""
import argparse, datetime, json, re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p=argparse.ArgumentParser();p.add_argument('--before',action='store_true');p.add_argument('--url',required=True);p.add_argument('--report',type=Path);a=p.parse_args()
here=Path(__file__).parent; checks=[]
def row(key,text,mode):
 return dict(id=key,createdAt=datetime.datetime.now().astimezone().isoformat(),rawTranscript=text,finalText=text,mode=mode,durationMs=1000,insertStatus='copiedFallback')
raw=row('raw-recent','The raw note I selected.','raw')
light=row('light-older','A different polished note.','light')
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  page=browser.new_page(viewport={'width':1180,'height':1000});page.set_default_timeout(6000)
  page.add_init_script("localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');window.__hf={rows:"+json.dumps([raw,light])+",copied:[]};Object.defineProperty(navigator,'clipboard',{value:{writeText:async text=>window.__hf.copied.push(text)},configurable:true});")
  def fixture(route):
   response=route.fetch();source=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert source.count(old)==1
   route.fulfill(response=response,body=source.replace(old,"if (!isTauri) {if(cmd==='list_history')return structuredClone(window.__hf.rows);return mock();}",1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
  page.goto(a.url,wait_until='networkidle')
  page.get_by_role('button',name='History',exact=True).click()
  # Locate the filter by its exact visible label, separate from card headings.
  page.get_by_role('button',name='Light polish',exact=True).click()
  page.get_by_role('button',name='Copy',exact=True).click();page.wait_for_function('window.__hf.copied.length===1')
  assert page.evaluate('window.__hf.copied[0]')==light['finalText']
  # Recent transcripts intentionally spans all modes, even while timeline is filtered.
  recent=page.get_by_role('button').filter(has_text=raw['finalText']).first
  recent.click();page.wait_for_timeout(100)
  page.get_by_role('button',name='Copy',exact=True).click();page.wait_for_function('window.__hf.copied.length===2')
  copied=page.evaluate('window.__hf.copied[1]')
  checks.append({'case':'recent-outside-active-filter','requested':raw['finalText'],'copied':copied,'correct':copied==raw['finalText']})
  assert (copied!=raw['finalText']) if a.before else (copied==raw['finalText'])
  if a.report:page.screenshot(path=str(a.report.with_suffix('.png')))
  if not a.before:
   timeline=page.locator('.ol-thinscroll > button[style*="box-shadow"]')
   selected=timeline.filter(has_text=raw['finalText'])
   assert selected.count()==1 and 'inset' in (selected.get_attribute('style') or '')
   checks.append({'case':'requested-recent-row-is-visible-and-highlighted','correct':True})
   page.get_by_role('button',name='Light polish',exact=True).click()
   selected=timeline.filter(has_text=light['finalText'])
   assert selected.count()==1 and 'inset' in (selected.get_attribute('style') or '')
   checks.append({'case':'filter-detail-and-highlight-agree','correct':True})
   # A recent entry already inside the filter must not discard that filter.
   page.get_by_role('button').filter(has_text=light['finalText']).first.click()
   assert timeline.filter(has_text=raw['finalText']).count()==0
   checks.append({'case':'same-mode-recent-selection-keeps-filter','correct':True})
 finally:browser.close()
report={'at':datetime.datetime.now().astimezone().isoformat(),'beforeCounterexample':a.before,'checks':checks,'passed':True,'scope':'Actual History frontend with controlled local records and clipboard; no native/user history, OS clipboard, cloud or speech/hardware claims.'}
if a.report:a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(report))
