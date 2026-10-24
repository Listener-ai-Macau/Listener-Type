"""Actual React translation guidance with a nondefault shortcut, five locales.
Isolated browser mocks only; no native app, hardware, model, service or clipboard.
"""
import argparse,datetime,json,re
from pathlib import Path
from playwright.sync_api import sync_playwright,expect

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--url',required=True);p.add_argument('--report',type=Path,required=True)
p.add_argument('--before',action='store_true');a=p.parse_args();cases=[]
with sync_playwright() as pw:
 browser=pw.chromium.launch(headless=True,channel='msedge')
 try:
  for locale in ['en','zh-CN','zh-TW','ja','ko']:
   page=browser.new_page(viewport={'width':1100,'height':1050})
   page.add_init_script("""
    localStorage.setItem('ol.locale',LOCALE);
    sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');
    sessionStorage.setItem('ol.blePairingPromptDeferredThisSession','1');
    window.__guidanceCalls=[];
    window.__guidanceFixture=(cmd,args,mock)=>{
     window.__guidanceCalls.push(cmd);
     const value=mock();
     if(cmd==='get_settings')return {...value,translationHotkey:{primary:'F9',modifiers:['ctrl','alt']},translationTargetLanguage:'English'};
     return value;
    };
   """.replace('LOCALE',json.dumps(locale)))
   def fixture(route):
    response=route.fetch();text=response.text();old='if (!isTauri) {\n    return mock();\n  }'
    assert old in text,'Actual IPC fixture changed; do not silently skip it'
    route.fulfill(response=response,body=text.replace(old,'if (!isTauri) {\n    return window.__guidanceFixture(cmd,args,mock);\n  }',1))
   page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
   page.goto(a.url,wait_until='networkidle')
   page.locator('aside nav button').nth(2).click();page.locator('[data-tool="translation"]').click()
   expect(page.locator('main')).to_contain_text('English')
   text=page.locator('main').inner_text();count=text.count('F9')
   case={'locale':locale,'configuredShortcut':'Ctrl+Alt+F9','renderedShortcutMentions':count,'obsoleteHardcodedShiftMentions':text.count('Shift')}
   if a.before:
    case['oldGuidanceDisagreesWithConfiguredShortcut']=count==0 and 'Shift' in text
    assert case['oldGuidanceDisagreesWithConfiguredShortcut']
   else:
    expect(page.locator('[data-translation-howto]')).to_contain_text('F9')
    expect(page.locator('[data-translation-device-howto]')).to_be_visible()
    expect(page.locator('[data-translation-mode-indicator]')).to_be_visible()
    assert count>=2 and 'Shift' not in text,'Guidance must follow the configured shortcut'
    commands=page.evaluate('window.__guidanceCalls')
    unexpected=[c for c in commands if 'download' in c or 'prepare' in c or c.startswith('set_selected_device')]
    if unexpected:
     a.report.with_suffix('.preparation-failure.json').write_text(json.dumps({'scope':'Read-only checker unexpected startup calls; not a product write qualification','locale':locale,'commands':commands},ensure_ascii=False,indent=2),encoding='utf8')
    assert not unexpected,unexpected
    case.update(deviceActionAndModeIndicatorExplained=True,nativePreferencesAndDeviceWrites=0,startupMockPreferenceWrites=commands.count('set_settings'),passed=True)
   cases.append(case);page.close()
 finally:browser.close()
r={'at':datetime.datetime.now().astimezone().isoformat(),'scope':__doc__,'before':a.before,'passed':True,'cases':cases}
a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n',encoding='utf8');print(json.dumps(r,ensure_ascii=False))
