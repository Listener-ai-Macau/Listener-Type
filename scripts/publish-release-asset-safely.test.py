"""Real release-state invariants under upload, promotion and lost-reply failures.
No network or product state. Old download must survive unsuccessful publication.
"""
import copy
import importlib.util
import unittest
from pathlib import Path

spec=importlib.util.spec_from_file_location('safe_asset',Path(__file__).with_name('publish-release-asset-safely.py'))
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
NAME='app.msi';SHA='a'*64
OLD={'id':1,'name':NAME,'state':'uploaded','size':42,'digest':'sha256:'+'b'*64}
NEW={'id':2,'name':'app.pending-'+SHA[:12]+'.msi','state':'uploaded','size':84,'digest':'sha256:'+SHA}

class API:
    def __init__(self):self.assets={1:copy.deepcopy(OLD)};self.events=[];self.failure=None
    def list(self):return copy.deepcopy(list(self.assets.values()))
    def get(self,id):
        self.events.append(('verify',id))
        item=copy.deepcopy(self.assets[id])
        if self.failure=='invalid-proof':item['digest']='sha256:wrong'
        return item
    def rename(self,id,name):
        self.events.append(('rename',id,name))
        if id==2 and name==NAME and self.failure=='before-promotion':raise OSError('network failure')
        self.assets[id]['name']=name
        if id==1 and name.endswith('.previous-1.msi') and self.failure=='old-rename-lost-reply':raise OSError('backup response lost')
        if id==2 and name==NAME and self.failure=='lost-reply':raise OSError('response lost')
    def delete(self,id):self.events.append(('delete',id));del self.assets[id]
    def upload(self,name):
        self.events.append(('upload',name))
        if self.failure=='upload':raise OSError('HTTP408')
        self.assets[2]=copy.deepcopy(NEW)
        if self.failure=='bad-upload':self.assets[2]['size']=1
        return copy.deepcopy(self.assets[2])

class Contracts(unittest.TestCase):
    def test_failed_upload_does_not_touch_old_download(self):
        for failure in ['upload','bad-upload']:
            api=API();api.failure=failure
            with self.assertRaises(Exception):m.promote_verified_asset(api,NAME,84,SHA,api.upload)
            self.assertEqual(api.assets[1],OLD)
            self.assertFalse(any(e[0] in ['rename','delete'] for e in api.events))
    def test_failed_promotion_or_bad_proof_restores_previous_name(self):
        for failure in ['before-promotion','invalid-proof','old-rename-lost-reply']:
            api=API();api.failure=failure
            with self.assertRaises(Exception):m.promote_verified_asset(api,NAME,84,SHA,api.upload)
            self.assertEqual(api.assets[1],OLD)
            self.assertFalse(any(e[0]=='delete' for e in api.events))
    def test_lost_success_reply_is_reconciled_from_authoritative_state(self):
        api=API();api.failure='lost-reply'
        result=m.promote_verified_asset(api,NAME,84,SHA,api.upload)
        self.assertTrue(result['complete']);self.assertEqual(api.assets[2]['name'],NAME)
        self.assertNotIn(1,api.assets)
    def test_old_is_removed_only_after_new_digest_verification(self):
        api=API();result=m.promote_verified_asset(api,NAME,84,SHA,api.upload)
        self.assertTrue(result['complete'])
        self.assertLess(api.events.index(('verify',2)),api.events.index(('delete',1)))
        self.assertEqual(len(api.assets),1)
    def test_completed_staging_reused_after_interruption(self):
        api=API();api.assets[2]=copy.deepcopy(NEW)
        m.promote_verified_asset(api,NAME,84,SHA,api.upload)
        self.assertFalse(any(e[0]=='upload' for e in api.events))
    def test_same_digest_already_published_is_not_uploaded_again(self):
        api=API();api.assets={2:dict(NEW,name=NAME)}
        result=m.promote_verified_asset(api,NAME,84,SHA,api.upload)
        self.assertTrue(result['alreadyCurrent']);self.assertEqual(api.events,[])

if __name__=='__main__':unittest.main()
