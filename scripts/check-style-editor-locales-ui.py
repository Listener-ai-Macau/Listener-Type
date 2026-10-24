"""Actual five-locale React style/vocabulary paths with explicit local IPC mocks.
No native persistence, service request, OS file dialog, hardware or Owner UI.
"""
import argparse,datetime,json,re,subprocess,traceback
from pathlib import Path
from playwright.sync_api import sync_playwright,expect
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
repo=Path(__file__).resolve().parents[1]
source=(repo/'src/i18n/styleEditor.ts').read_text(encoding='utf-8');decoder=json.JSONDecoder()
def messages(locale):
 marker='const en = ' if locale=='en' else json.dumps(locale)+': '
 return decoder.raw_decode(source[source.index(marker)+len(marker):])[0]
node="""import {en} from './src/i18n/en.ts';import {zhCN} from './src/i18n/zh-CN.ts';import {zhTW} from './src/i18n/zh-TW.ts';import {ja} from './src/i18n/ja.ts';import {ko} from './src/i18n/ko.ts';console.log(JSON.stringify(Object.fromEntries(Object.entries({en,'zh-CN':zhCN,'zh-TW':zhTW,ja,ko}).map(([l,m])=>[l,m.vocab.presets]))));"""
vocab=json.loads(subprocess.check_output(['node','--experimental-strip-types','--input-type=module','-e',node],cwd=repo,text=True,encoding='utf-8'))
names={'en':['Programmer','Chef · Chinese terms','Public administration · Chinese terms'],'zh-CN':['程序员','厨师','公务员'],'zh-TW':['程式設計師','廚師（中文詞條）','公務（中文詞條）'],'ja':['プログラマー','料理（中国語の用語）','行政（中国語の用語）'],'ko':['프로그래머','요리(중국어 용어)','행정(중국어 용어)']}
defaults=json.loads((repo/'src/lib/vocab-presets.json').read_text(encoding='utf-8'));checks=[]
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  for locale in ['en','zh-CN','zh-TW','ja','ko']:
   copy=messages(locale);page=browser.new_page(viewport={'width':1180,'height':900});page.set_default_timeout(8000)
   page.add_init_script("""localStorage.setItem('ol.locale',LOCALE);sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');window.__localesCalls=[];window.__styleReads=[];window.__localesFixture=(cmd,args,mock)=>{window.__localesCalls.push({cmd,args});const result=mock();if(cmd==='list_style_packs')window.__styleReads.push(structuredClone(result));return result;};""".replace('LOCALE',json.dumps(locale)))
   def route_ipc(route):
    response=route.fetch();text=response.text();old='if (!isTauri) {\n    return mock();\n  }';assert old in text
    route.fulfill(response=response,body=text.replace(old,'if (!isTauri) {\n    return window.__localesFixture(cmd,args,mock);\n  }',1))
   page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),route_ipc);page.goto(a.url,wait_until='networkidle')
   page.locator('aside nav button').nth(2).click();page.locator('[data-tool=style]').click()
   expect(page.get_by_role('heading',name=copy['title'],exact=True)).to_be_visible()
   expect(page.get_by_role('button',name=copy['importZip'],exact=True)).to_be_visible()
   page.get_by_role('button',name=re.compile('^'+re.escape(copy['addPackTileTitle']))).click()
   editor=page.locator('[role=dialog][aria-modal=true]');expect(editor).to_be_visible()
   name=editor.get_by_label(copy['fieldName'],exact=True);expect(name).to_have_value(copy['newName'])
   prompt=editor.get_by_role('textbox',name=re.compile(re.escape(copy['fullPromptTitle'])))
   expect(prompt).to_have_value(copy['newPrompt'])
   created=page.evaluate("window.__localesCalls.filter(x=>x.cmd==='create_style_pack_from_template')")
   assert len(created)==1 and created[0]['args']['template']['prompt']==copy['newPrompt']
   assert copy['newPrompt'].count('{{HOTWORDS}}')==1
   for width,height in [(1180,900),(900,700)]:
    page.set_viewport_size({'width':width,'height':height})
    assert editor.evaluate('(e)=>e.scrollWidth<=e.clientWidth+1')
    expect(name).to_have_value(copy['newName']);expect(prompt).to_have_value(copy['newPrompt'])
   page.set_viewport_size({'width':1180,'height':900})
   if locale in ['ja','ko','zh-TW']:page.screenshot(path=str(a.report.with_name(a.report.stem+'-'+locale+'-style.png')))
   owned='Owned 日本語 한국어 中文 {{name}}'
   name.fill(owned);editor.get_by_role('button',name=copy['save'],exact=True).click()
   page.wait_for_function("window.__localesCalls.some(x=>x.cmd==='save_style_pack'&&x.args.stylePack.name===NAME)".replace('NAME',json.dumps(owned)))
   editor.get_by_role('button',name=copy['closeEditor'],exact=True).click();expect(editor).to_have_count(0)
   expect(page.get_by_text(owned,exact=True).first).to_be_visible()
   reads=page.evaluate('window.__styleReads');before=reads[0]
   for old in before:
    kept=next(x for x in reads[-1] if x['id']==old['id'])
    assert all(kept[key]==old[key] for key in ['id','name','description','prompt','examples','tags','kind','baseMode'])
   checks.append({'locale':locale,'scope':'Localized editor/new template, literal hotword slot, explicit named save and original packs retained in local UI fixture','passed':True})
   page.locator('.ol-tools-back').click();page.locator('[data-tool=vocab]').click()
   page.get_by_role('button',name=re.compile('^'+re.escape(vocab[locale]['title']))).click()
   for label in names[locale]:expect(page.get_by_role('button',name=label,exact=True)).to_be_visible()
   page.get_by_role('button',name=vocab[locale]['edit'].replace('{{name}}',names[locale][1]),exact=True).click()
   preset_name=page.get_by_placeholder(vocab[locale]['namePlaceholder'],exact=True)
   phrases=page.get_by_placeholder(vocab[locale]['wordsPlaceholder'],exact=True)
   expect(preset_name).to_have_value(defaults[1]['name']);expect(phrases).to_have_value(', '.join(defaults[1]['phrases']))
   owned_preset='Own chef 日本語 {{HOTWORDS}}';preset_name.fill(owned_preset)
   page.get_by_role('button',name=vocab[locale]['save'],exact=True).click()
   expect(page.get_by_role('button',name=owned_preset,exact=True)).to_be_visible()
   writes=page.evaluate("window.__localesCalls.filter(x=>x.cmd==='save_vocab_presets')")
   saved=next(x for x in writes[-1]['args']['store']['overrides'] if x['id']=='chef')
   assert saved['name']==owned_preset and saved['phrases']==defaults[1]['phrases']
   page.get_by_role('button',name=owned_preset,exact=True).click()
   page.get_by_role('button',name=vocab[locale]['apply'],exact=True).click()
   page.wait_for_function("window.__localesCalls.filter(x=>x.cmd==='add_vocab').length===6")
   additions=page.evaluate("window.__localesCalls.filter(x=>x.cmd==='add_vocab').map(x=>x.args.phrase)")
   assert additions==defaults[1]['phrases'],additions
   assert not page.evaluate("window.__localesCalls.some(x=>/^(try_|suggest_|start_dictation|prepare_local|download_|set_embedded)/.test(x.cmd))")
   if locale in ['ja','ko','zh-TW']:page.screenshot(path=str(a.report.with_name(a.report.stem+'-'+locale+'.png')))
   checks.append({'locale':locale,'scope':'Factory labels localized, authored name/raw Chinese terms retained by edit/save/apply, no processing/download/device-write calls','passed':True})
   page.close()
 except Exception as error:
  a.report.parent.mkdir(parents=True,exist_ok=True)
  page.screenshot(path=str(a.report.with_name(a.report.stem+'-failure.png')))
  a.report.write_text(json.dumps({'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':False,'checks':checks,'error':str(error),'traceback':traceback.format_exc()},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
  raise
 finally:browser.close()
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'passed':True,'checks':checks};a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf-8');print(json.dumps(r,ensure_ascii=False))
