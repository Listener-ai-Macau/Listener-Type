import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { copyExplicitText, deleteSavedTextDraft, isTauri, listSavedTextDrafts, renameSavedTextDraft, type SavedTextDraft } from '../lib/ipc';
import { detectOS } from './WindowChrome';
import { Btn } from '../pages/_atoms';
import './DeviceLayoutsDialog.css';
import { SavedTextDraftCopyEditor } from './SavedTextDraftCopyEditor';
import { DeviceLayoutsDialog } from './DeviceLayoutsDialog';
import { writingDeviceKeyProfile } from '../lib/deviceKeyLayouts';

export function SavedTextDraftsDialog({onClose,initialDraftId,onUnavailable}:{onClose:()=>void;initialDraftId?:string;onUnavailable?:(id:string)=>void}) {
  const {t,i18n}=useTranslation();const dialog=useRef<HTMLDialogElement>(null),alive=useRef(true),running=useRef(false);
  // Keep the identity requested when this dialog opened, even if its owner
  // clears its saved receipt after learning that the record is unavailable.
  const requestedId=useRef(initialDraftId).current;
  const [items,setItems]=useState<SavedTextDraft[]>([]),[selectedId,setSelected]=useState(requestedId??'');
  const [missingRequested,setMissingRequested]=useState(false);
  const [loading,setLoading]=useState(true),[busy,setBusy]=useState(false),[loadError,setLoadError]=useState(false),[error,setError]=useState(''),[copied,setCopied]=useState(''),[confirm,setConfirm]=useState(false);
  const [editing,setEditing]=useState<SavedTextDraft|null>(null),[editText,setEditText]=useState(''),[editName,setEditName]=useState(''),[editBaseline,setEditBaseline]=useState('');
  const [leaveTarget,setLeaveTarget]=useState<'dialog'|'editor'|null>(null);
  const leaveConfirmation=useRef<HTMLDivElement>(null);
  useEffect(()=>{
    if(!leaveTarget)return;
    leaveConfirmation.current?.scrollIntoView({block:'nearest'});
    leaveConfirmation.current?.querySelector('button')?.focus({preventScroll:true});
  },[leaveTarget]);
  const [keyPhrase,setKeyPhrase]=useState<string|null>(null);
  const [nameEdit,setNameEdit]=useState(''),[nameError,setNameError]=useState(''),[namedId,setNamedId]=useState('');
  const selected=items.find(d=>d.id===selectedId);
  const nameDirty=!!selected&&!editing&&nameEdit!==selected.name;
  // A conflict refresh updates the expected name, but keeps the user's edit.
  useEffect(()=>{setNameEdit(selected?.name??'');setNameError('');setNamedId('');},[selected?.id]);
  const draftLabel=(draft:SavedTextDraft)=>{
    const text=Array.from(draft.text.replace(/\s+/g,' ').trim());
    // A revised deadline often sits at the end of an otherwise identical
    // message. Keep both ends within the same bounded label, rather than
    // permanently hiding every character after the shared opening.
    const preview=text.length>55?text.slice(0,24).join('')+'…'+text.slice(-30).join(''):text.join('');
    const time=new Date(draft.createdAt).toLocaleString(i18n.resolvedLanguage??i18n.language,{year:'numeric',month:'short',day:'numeric',hour:'2-digit',minute:'2-digit'});
    return `${draft.name} · ${preview} · ${time}`;
  };
  const choices=useMemo(()=>items.map(d=>({id:d.id,label:draftLabel(d)})),[items,i18n.resolvedLanguage,i18n.language]);
  const refresh=async()=>{setLoading(true);setLoadError(false);try{const value=await listSavedTextDrafts();if(alive.current){const missing=!!requestedId&&!value.some(d=>d.id===requestedId);setItems(value);setMissingRequested(missing);setSelected(current=>value.some(d=>d.id===current)?current:missing?'':value[0]?.id??'');if(missing)onUnavailable?.(requestedId!);}}catch{if(alive.current)setLoadError(true);}finally{if(alive.current)setLoading(false);}};
  useEffect(()=>{alive.current=true;dialog.current?.showModal();void refresh();return()=>{alive.current=false;dialog.current?.close();};},[]);
  const copy=async()=>{if(!selected||running.current)return;const value=selected;running.current=true;setBusy(true);setError('');setCopied('');try{await copyExplicitText(value.text,detectOS()==='win');if(alive.current)setCopied(value.id);}catch{if(alive.current)setError(t('textDraft.copyFailed'));}finally{running.current=false;if(alive.current)setBusy(false);}};
  const remove=async()=>{if(!selected||running.current)return;const id=selected.id;running.current=true;setBusy(true);setError('');try{await deleteSavedTextDraft(id);if(alive.current){setItems(current=>current.filter(d=>d.id!==id));setSelected(items.find(d=>d.id!==id)?.id??'');setConfirm(false);setCopied('');onUnavailable?.(id);}}catch{if(alive.current)setError(t('textDraft.deleteFailed'));}finally{running.current=false;if(alive.current)setBusy(false);}};
  const rename=async()=>{
    if(!selected||running.current||!nameDirty)return;
    const target=selected,name=nameEdit.trim();
    if(!name||Array.from(name).length>80){setNameError('textDraft.nameInvalid');return;}
    running.current=true;setBusy(true);setNameError('');setNamedId('');
    try{
      const updated=await renameSavedTextDraft(target.id,target.name,name);
      if(alive.current){setItems(current=>current.map(d=>d.id===updated.id?updated:d));setNameEdit(updated.name);setNamedId(updated.id);}
    }catch(cause){
      if(alive.current){
        const message=String(cause);
        if(message.includes('saved_draft_missing')){setItems(current=>current.filter(d=>d.id!==target.id));setSelected('');setMissingRequested(true);setNameEdit('');setError(t('textDraft.nameMissing'));onUnavailable?.(target.id);}
        else setNameError(message.includes('saved_draft_name_changed')?'textDraft.nameChanged':message.includes('saved_draft_invalid_name')?'textDraft.nameInvalid':'textDraft.nameFailed');
      }
    }finally{running.current=false;if(alive.current)setBusy(false);}
  };
  const finishLeave=(target:'dialog'|'editor')=>{setLeaveTarget(null);if(target==='dialog')onClose();else{setEditing(null);setEditText('');setEditName('');setEditBaseline('');}};
  const requestLeave=(target:'dialog'|'editor')=>{if(running.current||busy)return;if(nameDirty||(editing&&JSON.stringify([editName.trim(),editText])!==editBaseline))setLeaveTarget(target);else finishLeave(target);};
  const close=()=>requestLeave('dialog');
  return <><dialog ref={dialog} className="ol-device-layout-dialog ol-thinscroll" aria-labelledby="saved-text-drafts-title" onCancel={e=>{e.preventDefault();close();}}>
    <div className="ol-device-layout-heading"><h2 id="saved-text-drafts-title">{t('textDraft.title')}</h2><Btn size="sm" variant="ghost" disabled={busy} onClick={close}>{t('common.close')}</Btn></div>
    <p className="ol-device-layout-hint">{t(isTauri?'textWorkspace.draftsIntro':'textDraft.browser')}</p>
    {missingRequested&&<p role="status">{t('textDraft.missingSelected')}</p>}
    {loading?<p role="status">{t('common.loading')}</p>:loadError?<p role="alert">{t('textDraft.loadFailed')} <Btn size="sm" onClick={()=>void refresh()}>{t('common.retry')}</Btn></p>:!items.length?<p>{t('textDraft.empty')}</p>:<>
      <label>{t('textDraft.choose')}<select aria-label={t('textDraft.choose')} value={selectedId} disabled={busy||!!editing||nameDirty} onChange={e=>{setSelected(e.target.value);setMissingRequested(false);setConfirm(false);setError('');setCopied('');}}>
        {missingRequested&&<option value="" disabled>{t('textDraft.choose')}</option>}
        {choices.map(d=><option key={d.id} value={d.id}>{d.label}</option>)}
      </select></label>
      {editing?<SavedTextDraftCopyEditor record={editing} name={editName} text={editText} disabled={!!leaveTarget}
        onNameChange={setEditName} onTextChange={setEditText} onBusy={setBusy}
        onCreated={record=>{setItems(current=>[record,...current.filter(d=>d.id!==record.id)]);setSelected(record.id);setEditBaseline(JSON.stringify([record.name,record.text]));}}
        onBack={()=>requestLeave('editor')}/>:selected&&<section aria-label={t('textDraft.selected')}>
        <div style={{marginTop:14}} data-saved-draft-name>
          <label>{t('textDraft.nameLabel')}<input type="text" aria-label={t('textDraft.nameLabel')} value={nameEdit} maxLength={80}
            disabled={busy||!!leaveTarget} style={{width:'100%'}}
            onChange={e=>{setNameEdit(e.target.value);setNameError('');setNamedId('');setConfirm(false);}}
            onKeyDown={e=>{if(e.key==='Enter'){e.preventDefault();void rename();}}}/></label>
          <p className="ol-device-layout-hint">{t('textDraft.nameHint')}</p>
          {nameDirty&&<div style={{display:'flex',gap:10,flexWrap:'wrap'}}>
            <Btn size="sm" disabled={busy||!!leaveTarget||!nameEdit.trim()} onClick={()=>void rename()}>{t(busy?'textDraft.saving':'textDraft.saveName')}</Btn>
            <Btn size="sm" variant="ghost" disabled={busy||!!leaveTarget} onClick={()=>{setNameEdit(selected.name);setNameError('');setNamedId('');}}>{t('common.cancel')}</Btn>
          </div>}
          {namedId===selected.id&&!nameDirty&&<p role="status">{t('textDraft.nameSaved')}</p>}
          {nameError&&<p role="alert">{t(nameError)} {nameError==='textDraft.nameChanged'&&<Btn size="sm" variant="ghost" disabled={busy} onClick={()=>{setNameError('');void refresh();}}>{t('textDraft.reloadName')}</Btn>}</p>}
        </div>
        <textarea readOnly aria-label={t('textDraft.selected')} value={selected.text} rows={9} style={{width:'100%',lineHeight:1.6,resize:'vertical',marginTop:14}}/>
        <details style={{margin:'14px 0'}}><summary>{t('textDraft.original')}</summary><pre className="ol-saved-phrase-text">{selected.original}</pre></details>
        <div style={{display:'flex',gap:10,flexWrap:'wrap'}}><Btn size="sm" disabled={busy} onClick={()=>void copy()}>{t(busy?'textDraft.working':copied===selected.id?'textDraft.copied':'textDraft.copy')}</Btn>
          <Btn size="sm" variant="ghost" disabled={busy||nameDirty} onClick={()=>{const name=Array.from(t('textDraft.copyName',{name:selected.name})).slice(0,80).join('').trim();setEditing({...selected});setEditText(selected.text);setEditName(name);setEditBaseline(JSON.stringify([name,selected.text]));setConfirm(false);setCopied('');setError('');}}>{t('textDraft.editCopy')}</Btn>
          <Btn size="sm" variant="ghost" disabled={busy||nameDirty||!selected.text.trim()||selected.text.length>2000} onClick={()=>setKeyPhrase(selected.text)}>{t('keyPhrase.open')}</Btn>
          <Btn size="sm" variant="ghost" disabled={busy||nameDirty} onClick={()=>setConfirm(true)}>{t('textDraft.delete')}</Btn></div>
        {selected.text.length>2000&&<p className="ol-device-layout-hint">{t('keyPhrase.tooLong')}</p>}
        {copied===selected.id&&<p role="status">{t('textDraft.paste')}</p>}
        {confirm&&<div role="alert"><p>{t('textDraft.confirm')}</p><Btn size="sm" variant="ghost" disabled={busy} onClick={()=>setConfirm(false)}>{t('common.cancel')}</Btn> <Btn size="sm" disabled={busy} onClick={()=>void remove()}>{t('textDraft.delete')}</Btn></div>}
      </section>}
    </>}
    {leaveTarget&&<div ref={leaveConfirmation} role="alert" className="ol-device-layout-confirm"><p>{t(nameDirty?'textDraft.discardNameConfirm':'textDraft.discardCopyConfirm')}</p>
      <Btn size="sm" variant="ghost" onClick={()=>setLeaveTarget(null)}>{t('common.cancel')}</Btn>{' '}
      <Btn size="sm" onClick={()=>finishLeave(leaveTarget)}>{t('textDraft.discardCopy')}</Btn>
    </div>}
    {error&&<p role="alert">{error}</p>}
    <details className="ol-style-trial-details"><summary>{t('textWorkspace.draftsDetails')}</summary><p className="ol-device-layout-hint">{t('textDraft.intro')}</p></details>
  </dialog>{keyPhrase!==null&&<DeviceLayoutsDialog rememberSelection initialProfile={writingDeviceKeyProfile()}
    incomingText={keyPhrase} phraseOrigin="savedDraft" onLeaveForPage={onClose} onClose={()=>setKeyPhrase(null)}/>}</>;
}
