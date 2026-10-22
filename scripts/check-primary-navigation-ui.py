"""Exercise the real React navigation in a headless browser with browser mocks.
No desktop focus, real device, credentials, recording or service calls.
"""
import argparse,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect

p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
report={'scope':__doc__,'passed':False,'cases':[]};a.report.parent.mkdir(parents=True,exist_ok=True)
labels={'en':['Overview','History','Tools'],'zh-CN':['概览','历史','工具'],'zh-TW':['概覽','歷史','工具'],'ja':['概要','履歴','ツール'],'ko':['개요','기록','도구']}
def settled(page):
 # The content remains mounted through its exit animation. A card count alone
 # could mistake the previous page for the destination and capture a blank fade.
 page.wait_for_function("""() => {
  const content = document.querySelector('main > .ol-thinscroll');
  return content && getComputedStyle(content).opacity === '1'
   && !content.getAnimations().some(a => a.playState === 'running')
   && !content.style.animation.includes('fadeout');
 }""")
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  for locale,names in labels.items():
   context=browser.new_context(viewport={'width':1280,'height':1000})
   context.route(re.compile(r'^https?://'),lambda r:r.continue_() if re.match(r'^https?://(127\.0\.0\.1|localhost)(:|/)',r.request.url) else r.abort())
   page=context.new_page();page.set_default_timeout(7000)
   page.add_init_script("localStorage.setItem('ol.locale',"+json.dumps(locale)+");sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');")
   page.goto(a.url,wait_until='networkidle')
   nav=page.locator('aside nav');expect(nav.get_by_role('button')).to_have_count(3)
   actual=nav.get_by_role('button').all_text_contents()
   assert [v.strip() for v in actual]==names,{'locale':locale,'actual':actual,'expected':names}
   nav.get_by_role('button',name=names[2],exact=True).click()
   expect(page.locator('[data-tool]')).to_have_count(4)
   settled(page)
   reached=[]
   for tool in ('style','vocab','translation','selectionAsk'):
    page.locator('[data-tool="'+tool+'"]').click()
    expect(page.locator('.ol-tools-back')).to_be_visible()
    expect(nav.locator('.ol-nav-btn-active')).to_have_text(names[2])
    expect(page.locator('[data-tool]')).to_have_count(0)
    settled(page)
    page.locator('.ol-tools-back').click()
    expect(page.locator('[data-tool]')).to_have_count(4)
    settled(page)
    reached.append(tool)
   nav.get_by_role('button',name=names[1],exact=True).click()
   expect(nav.locator('.ol-nav-btn-active')).to_have_text(names[1])
   settled(page)
   nav.get_by_role('button',name=names[0],exact=True).click()
   expect(nav.locator('.ol-nav-btn-active')).to_have_text(names[0])
   settled(page)
   report['cases'].append({'locale':locale,'primary':actual,'allFourToolsOpenAndReturn':reached})
   if locale=='zh-CN':
    nav.get_by_role('button',name=names[2],exact=True).click();expect(page.locator('[data-tool]')).to_have_count(4)
    settled(page)
    page.screenshot(path=str(a.report.with_suffix('.png')))
    page.set_viewport_size({'width':820,'height':720})
    assert page.evaluate('document.documentElement.scrollWidth<=window.innerWidth')
    for card in page.locator('[data-tool]').all():
     box=card.bounding_box();assert box and box['x']>=0 and box['x']+box['width']<=820
    report['narrowWidthNoHorizontalClipping']=True
    page.screenshot(path=str(a.report.with_name(a.report.stem+'-narrow.png')))
   context.close()
  report['passed']=True
 finally:
  browser.close();a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))
