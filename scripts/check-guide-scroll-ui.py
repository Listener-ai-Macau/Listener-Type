import argparse,re
parser=argparse.ArgumentParser();parser.add_argument("--url",required=True);args=parser.parse_args()
from playwright.sync_api import sync_playwright,expect
with sync_playwright() as p:
 b=p.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu'])
 try:
  for width,height in [(1240,800),(360,640)]:
   page=b.new_page(viewport={'width':width,'height':height});page.add_init_script("localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');window.__fixture=(cmd,args,mock)=>cmd==='get_credentials'?{...mock(),llmConfigured:false}:mock();")
   def fixture(route):
    r=route.fetch();s=r.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1;route.fulfill(response=r,body=s.replace(old,'if (!isTauri) {\n    return window.__fixture(cmd,args,mock);\n  }',1))
   page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
   page.goto(args.url,wait_until='networkidle');guide=page.get_by_role('button',name='Usage guide',exact=True);guide.scroll_into_view_if_needed();scroller=guide.locator('xpath=ancestor::div[contains(@class,"ol-thinscroll")][1]')
   before=scroller.evaluate('(e)=>e.scrollTop');assert before>0,(width,before);guide.click()
   banner=page.get_by_role('status').filter(has_text='Dictation uses your configured speech service');expect(banner).to_be_in_viewport();assert scroller.evaluate('(e)=>e.scrollTop')==0
   expect(banner.get_by_role('button',name='Close usage guide')).to_be_visible();banner.get_by_role('button',name='Close usage guide').click();expect(banner).to_have_count(0)
   guide.scroll_into_view_if_needed();assert scroller.evaluate('(e)=>e.scrollTop')>0;guide.click();expect(banner).to_be_in_viewport();assert scroller.evaluate('(e)=>e.scrollTop')==0
   print(f'PASS viewport {width}x{height}: scrolled trigger opens guide visibly; close and reopen work')
   page.close()
 finally:b.close()
