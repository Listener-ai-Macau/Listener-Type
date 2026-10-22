"""Owned headless React checks: first-use dismissal, restart and recoverable tools.
Browser mocks only, no daily desktop, actual service, device or clipboard use.
"""
import argparse,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
result={'scope':__doc__,'passed':False,'cases':[]}
def settled(page):
 page.wait_for_function("""()=>{let e=document.querySelector('main > .ol-thinscroll');return e&&getComputedStyle(e).opacity==='1'&&!e.style.animation.includes('fadeout')&&!e.getAnimations().some(a=>a.playState==='running');}""")
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  for locale in ['en','zh-CN','zh-TW','ja','ko']:
   context=browser.new_context(viewport={'width':1280,'height':1000})
   context.route(re.compile(r'^https?://'),lambda r:r.continue_() if re.match(r'^https?://(127\.0\.0\.1|localhost)(:|/)',r.request.url) else r.abort())
   page=context.new_page();page.set_default_timeout(7000)
   page.add_init_script("localStorage.setItem('ol.locale',"+json.dumps(locale)+");sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');")
   page.goto(a.url,wait_until='networkidle')
   nav=page.locator('aside nav');expect(nav.locator('button')).to_have_count(3);settled(page)
   guide_title=page.locator('main > .ol-thinscroll h1').inner_text()
   skip=page.locator('main > .ol-thinscroll > div').first.get_by_role('button')
   expect(skip).to_have_count(1);assert 'firstUse.' not in skip.inner_text()
   skip.click();expect(nav.locator('button')).to_have_count(2)
   expect(page.locator('main > .ol-thinscroll h1')).not_to_have_text(guide_title);settled(page)
   assert page.evaluate("localStorage.getItem('ol.overviewHidden.v1')")=='1'
   expect(nav.locator('button').first).to_have_class(re.compile('ol-nav-btn-active'))
   page.reload(wait_until='networkidle');expect(nav.locator('button')).to_have_count(2);settled(page)
   expect(nav.locator('button').first).to_have_class(re.compile('ol-nav-btn-active'))
   page.locator('[data-open-overview]').click();expect(page.locator('main > .ol-thinscroll h1')).to_have_text(guide_title);settled(page)
   expect(nav.locator('button')).to_have_count(2);expect(nav.locator('.ol-nav-btn-active')).to_have_count(0)
   assert page.locator('aside nav > div').count()==0,'no stale active pill on recovered guide'
   page.locator('main > .ol-thinscroll > div').first.get_by_role('button').click()
   expect(page.locator('main > .ol-thinscroll h1')).not_to_have_text(guide_title);settled(page)
   nav.locator('button').last.click();expect(page.locator('[data-tool]')).to_have_count(4);settled(page)
   expect(page.locator('details')).not_to_have_attribute('open','')
   page.locator('summary').click();expect(page.locator('details')).to_have_attribute('open','')
   expect(page.locator('details button')).to_have_count(6)
   if locale=='en':
    for label in ['Try typed text','Saved drafts','Transcribe recorded audio','Design your keys','Try the four keys','Use my saved phrases']:
     page.locator('details').get_by_role('button',name=label,exact=True).click()
     # Some existing application sheets use custom divs. Their visible Close
     # button is the customer control, independent of a particular dialog tag.
     close=page.get_by_role('button',name='Close',exact=True).last;expect(close).to_be_visible();close.click()
   page.set_viewport_size({'width':820,'height':720});settled(page)
   assert page.evaluate('document.documentElement.scrollWidth<=innerWidth')
   if locale=='zh-CN':page.screenshot(path=str(a.report.with_suffix('.png')))
   result['cases'].append({'locale':locale,'skipPersistsAcrossReload':True,'recoverGuideWithoutRestoringPrimaryTab':True,'sixExistingUtilitiesRetained':True,'narrowWidth':820})
   context.close()
  # Failed persistence hides only the current session and says so explicitly.
  context=browser.new_context();page=context.new_page()
  page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');const set=Storage.prototype.setItem;Storage.prototype.setItem=function(k,v){if(k==='ol.overviewHidden.v1')throw new Error('storage denied');return set.call(this,k,v);};""")
  page.goto(a.url,wait_until='networkidle');page.get_by_role('button',name='Skip and don’t show again',exact=True).click()
  expect(page.locator('aside nav button')).to_have_count(2);expect(page.get_by_role('status')).to_contain_text('could not save the choice')
  page.reload(wait_until='networkidle');expect(page.locator('aside nav button')).to_have_count(3)
  result['failedPersistenceIsHonestAndDoesNotRemoveGuide']=True;context.close();result['passed']=True
 finally:
  browser.close();a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(result,ensure_ascii=False))
