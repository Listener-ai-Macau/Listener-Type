"""Controlled local translation: explicit term preservation and fail-closed identity."""
import argparse,re
from playwright.sync_api import sync_playwright,expect
parser=argparse.ArgumentParser();parser.add_argument('--url',required=True);args=parser.parse_args()
with sync_playwright() as p:
 b=p.chromium.launch(headless=True,channel='msedge',args=['--disable-gpu'])
 try:
  page=b.new_page(viewport={'width':1180,'height':900})
  page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');window.__calls=[];window.__bad=false;window.__fixture=(cmd,args,mock)=>{if(cmd==='get_local_translation_status')return {supported:true,runtimeReady:true,runtimeDownloadBytes:0,pairs:[{source:'zh',target:'en',downloadBytes:0,ready:true},{source:'en',target:'zh',downloadBytes:0,ready:true}],preparing:false,receivedBytes:0,expectedBytes:0};if(cmd==='try_local_translation_text'){window.__calls.push(args);return {packId:'local_translation:zh:en',packName:'local',outputText:window.__bad?'The name was lost.':args.rawText.includes('ZXQKEEP1ZXQ')?'ZXQKEEP0ZXQ, please confirm ZXQKEEP1ZXQ.':'ZXQKEEP0ZXQ, please confirm.',usedTextService:false,elapsedMs:10};}if(cmd==='get_style_text_trial_readiness')return {available:true,requiresTextService:false,textServiceConfigured:false};if(cmd==='try_translation_text')throw Error('unexpected cloud call');return mock();};""")
  def fixture(route):
   r=route.fetch();s=r.text();old='if (!isTauri) {\n    return mock();\n  }';assert s.count(old)==1;route.fulfill(response=r,body=s.replace(old,'if (!isTauri) {\n    return window.__fixture(cmd,args,mock);\n  }',1))
  page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture);page.goto(args.url,wait_until='networkidle')
  page.get_by_role('button',name='Chinese ↔ English on this PC',exact=True).click();d=page.get_by_role('dialog')
  source=d.locator('.ol-style-trial-input');source.fill('小林，请确认。');d.get_by_role('button',name='Use Chinese as the source',exact=True).click()
  box=d.locator('[data-protected-translation-terms]');box.locator('summary').click();box.get_by_role('textbox').fill('小林')
  assert page.evaluate('window.__calls.length')==0
  d.get_by_role('button',name='Translate',exact=True).click();result=d.get_by_role('textbox',name='Result text',exact=True)
  expect(result).to_have_value('小林, please confirm.');expect(source).to_have_value('小林，请确认。')
  calls=page.evaluate('window.__calls');assert len(calls)==1 and calls[0]['rawText']=='ZXQKEEP0ZXQ，请确认。'
  d.locator('[data-text-options] summary').click() if d.locator('[data-text-options]').count() else None
  if not box.is_visible():page.locator('details.ol-style-trial-options-panel > summary').click()
  if not box.evaluate('e=>e.open'):box.locator('summary').click()
  source.fill('小林，请确认报价。');box.get_by_role('textbox').fill('小林\n报价 -> quotation')
  d.get_by_role('button',name='Translate',exact=True).click();expect(result).to_have_value('小林, please confirm quotation.')
  expect(source).to_have_value('小林，请确认报价。');assert len(page.evaluate('window.__calls'))==2
  if not box.is_visible():page.locator('details.ol-style-trial-options-panel > summary').click()
  # Changing the term list invalidates the old result; a lost marker must not be adopted.
  if not box.is_visible():
   page.locator('details.ol-style-trial-options-panel > summary').click()
  if not box.evaluate('e=>e.open'):box.locator('summary').click()
  box.get_by_role('textbox').fill('小林\n不存在');page.evaluate('window.__bad=true')
  d.get_by_role('button',name='Translate',exact=True).click();expect(d.get_by_role('alert')).to_contain_text('did not preserve every selected term')
  expect(result).to_have_count(0);expect(source).to_have_value('小林，请确认报价。')
  assert len(page.evaluate('window.__calls'))==3
  print('PASS explicit name and preferred translation restore, changed terms invalidate old result, and missing marker rejects without cloud')
 finally:b.close()
