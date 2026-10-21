"""Measure the actual customer-observed trial control movement in an owned headless UI."""
import argparse, json, re
from datetime import datetime
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

p=argparse.ArgumentParser()
p.add_argument('--url',required=True)
p.add_argument('--report',required=True,type=Path)
p.add_argument('--width',type=int,default=1280)
p.add_argument('--height',type=int,default=900)
a=p.parse_args()
report={'at':datetime.now().astimezone().isoformat(),'scope':'Controlled private frontend, delayed readiness and normal style selection, no cloud, native app, clipboard or hardware.'}
with sync_playwright() as pw:
    browser=pw.chromium.launch(headless=True,channel='msedge')
    try:
        page=browser.new_page(viewport={'width':a.width,'height':a.height})
        page.add_init_script("""localStorage.setItem('ol.locale','en');sessionStorage.setItem('ol.providerSetupPromptDeferredThisSession','1');window.__layoutFixture=(cmd,args,mock)=>cmd==='get_style_text_trial_readiness'?new Promise(resolve=>window.__resolveLayout=resolve):mock();""")
        def fixture(route):
            r=route.fetch();s=r.text();old='if (!isTauri) {\n    return mock();\n  }'
            assert s.count(old)==1
            route.fulfill(response=r,body=s.replace(old,'if (!isTauri) {\n    return window.__layoutFixture(cmd,args,mock);\n  }',1))
        page.route(re.compile(r'/src/lib/ipc\.ts(?:\?.*)?$'),fixture)
        page.goto(a.url,wait_until='networkidle')
        page.get_by_role('button',name='Try typed text',exact=True).click()
        dialog=page.get_by_role('dialog')
        run=dialog.get_by_role('button',name='Try this text',exact=True)
        entry=dialog.get_by_label('Your text',exact=True)
        expect(dialog.get_by_role('status')).to_contain_text('Checking trial availability')
        entry.fill('Keep the original meaning.')
        entry.focus()
        samples={}
        def capture(name):
            samples[name]={'dialog':dialog.bounding_box(),'input':entry.bounding_box(),'run':run.bounding_box(),'draft':entry.input_value(),'inputFocused':entry.evaluate('(e)=>document.activeElement===e')}
        capture('rawChecking')
        page.evaluate('window.__resolveLayout({available:true,requiresTextService:false,textServiceConfigured:true})')
        expect(run).to_be_enabled()
        capture('rawReady')
        dialog.get_by_role('combobox',name='Trial style').select_option('builtin.light')
        expect(dialog.get_by_role('status')).to_contain_text('Checking trial availability')
        capture('lightChecking')
        page.evaluate('window.__resolveLayout({available:true,requiresTextService:true,textServiceConfigured:true})')
        expect(run).to_be_enabled()
        capture('lightReady')
        report['samples']=samples
        report['rawReadinessButtonMovementPx']=samples['rawReady']['run']['y']-samples['rawChecking']['run']['y']
        report['lightReadinessButtonMovementPx']=samples['lightReady']['run']['y']-samples['lightChecking']['run']['y']
        report['styleButtonMovementPx']=samples['lightReady']['run']['y']-samples['rawReady']['run']['y']
        report['maximumControlMovementPx']=max(abs(samples[name][key][axis]-samples['rawChecking'][key][axis]) for name in samples for key in ('input','run') for axis in ('x','y'))
        report['draftPreserved']=all(s['draft']=='Keep the original meaning.' for s in samples.values())
        report['dialogWithinViewport']=all(s['dialog']['x']>=0 and s['dialog']['y']>=0 and s['dialog']['x']+s['dialog']['width']<=a.width+1 and s['dialog']['y']+s['dialog']['height']<=a.height+1 for s in samples.values())
        page.screenshot(path=str(a.report.with_suffix('.png')))
        report['measured']=True
    finally:browser.close()
a.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))

assert report['maximumControlMovementPx']<1 and report['draftPreserved'] and report['dialogWithinViewport'], 'Trial controls moved or overlay exceeded viewport'
