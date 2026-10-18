"""Native regression of the actual scope bug and the shared repair primitive."""
import csv,datetime,io,json,subprocess,sys,uuid
from pathlib import Path
import windows_credential_scope as store

if len(sys.argv)>2 and sys.argv[1]=='--read-fixture':
    target=sys.argv[2]
    assert target.startswith('com.listener.type.test.scope.')
    found=store.read(target)
    print(json.dumps({'value':found['blob'].decode('utf-16-le'),'persist':found['persist']}))
    raise SystemExit()

# A different persistence write can switch the OS view for OTHER targets.
# A unique fixture name alone does not isolate a running application's reads.
# Never reproduce the negative case on a workstation where Type is in use.
processes=csv.reader(io.StringIO(subprocess.check_output(
    ['tasklist','/FI','IMAGENAME eq listener-type.exe','/FO','CSV','/NH'],text=True)))
if any(row and row[0].lower()=='listener-type.exe' for row in processes):
    raise SystemExit('Refusing the negative scope-switch experiment while Type is running; use an isolated CI/user session. Do not close the user app for this test.')

target='com.listener.type.test.scope.'+uuid.uuid4().hex
other=target+'.other'
old='fixture-old';new='fixture-new'
before=[];after=[]
try:
    store._write(target,old.encode('utf-16-le'),3)
    store._write(target,new.encode('utf-16-le'),2)
    for persistence in (3,2):
        store._write(other,b'fixture-switch',persistence)
        before.append({'otherPersistence':persistence,'targetValue':store.read(target)['blob'].decode('utf-16-le')})
    assert [x['targetValue'] for x in before]==[old,new], 'Original split-store defect must actually reproduce'
    store.canonicalize(target,new.encode('utf-16-le'))
    for persistence in (3,2,3,2):
        store._write(other,b'fixture-switch',persistence)
        selected=store.read(target)
        fresh=json.loads(subprocess.check_output([sys.executable,__file__,'--read-fixture',target],text=True))
        assert selected['blob'].decode('utf-16-le')==fresh['value']==new
        assert selected['persist']==fresh['persist']==3
        after.append({'otherPersistence':persistence,'currentAndFreshProcessRetainNew':True,'selectedPersistence':3})
    report={'at':datetime.datetime.now().astimezone().isoformat(),'before':before,'after':after,
            'testNamespaceOnly':True,'productCredentialsUsed':False,'passed':True}
    print(json.dumps(report))
finally:
    for entry in (other,target):store.delete_fixture(entry)
