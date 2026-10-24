import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { copyExplicitText, deleteSavedTextDraft, isTauri, listSavedTextDrafts, type SavedTextDraft } from '../lib/ipc';
import { detectOS } from './WindowChrome';
import { Btn } from '../pages/_atoms';
import './DeviceLayoutsDialog.css';
import { SavedTextDraftCopyEditor } from './SavedTextDraftCopyEditor';

export function SavedTextDraftsDialog({onClose,initialDraftId,onUnavailable}:{onClose:()=>void;initialDraftId?:string;onUnavailable?:(id:string)=>void}) {
  const {t,i18n}=useTranslation();const dialog=useRef<HTMLDialogElement>(null),alive=useRef(true),running=useRef(false);
  // Keep the identity requested when this dialog opened, even if its owner
  // clears its saved receipt after learning that the record is unavailable.
  const requestedId=useRef(initialDraftId).current;
  const [items,setItems]=useState<SavedTextDraft[]>([]),[selectedId,setSelected]=useState(requestedId??'');
  const [missingRequested,setMissingRequested]=useState(false);
  const [loading,setLoading]=useState(true),[busy,setBusy]=useState(false),[loadError,setLoadError]=useState(false),[error,setError]=useState(''),[copied,setCopied]=useState(''),[confirm,setConfirm]=useState(false);
  const [editing,setEditing]=useState<SavedTextDraft|null>(null),[editText,setEditText]=useState(''),[editSaved,setEditSaved]=useState<string|null>(null);
  const [leaveTarget,setLeaveTarget]=useState<'dialog'|'editor'|null>(null);
  const selected=items.find(d=>d.id===selectedId);
  const draftLabel=(draft:SavedTextDraft)=>{
    const text=Array.from(draft.text.replace(/\s+/g,' ').trim());
    const preview=text.slice(0,55).join('')+(text.length>55?'…':'');
    const time=new Date(draft.createdAt).toLocaleString(i18n.resolvedLanguage??i18n.language,{year:'numeric',month:'short',day:'numeric',hour:'2-digit',minute:'2-digit'});
    return `${preview} · ${draft.name} · ${time}`;
  };
  const refresh=async()=>{setLoading(true);setLoadError(false);try{const value=await listSavedTextDrafts();if(alive.current){const missing=!!requestedId&&!value.some(d=>d.id===requestedId);setItems(value);setMissingRequested(missing);setSelected(current=>value.some(d=>d.id===current)?current:missing?'':value[0]?.id??'');if(missing)onUnavailable?.(requestedId!);}}catch{if(alive.current)setLoadError(true);}finally{if(alive.current)setLoading(false);}};
  useEffect(()=>{alive.current=true;dialog.current?.showModal();void refresh();return()=>{alive.current=false;dialog.current?.close();};},[]);
  const copy=async()=>{if(!selected||running.current)return;const value=selected;running.current=true;setBusy(true);setError('');setCopied('');try{await copyExplicitText(value.text,detectOS()==='win');if(alive.current)setCopied(value.id);}catch{if(alive.current)setError(t('textDraft.copyFailed'));}finally{running.current=false;if(alive.current)setBusy(false);}};
  const remove=async()=>{if(!selected||running.current)return;const id=selected.id;running.current=true;setBusy(true);setError('');try{await deleteSavedTextDraft(id);if(alive.current){setItems(current=>current.filter(d=>d.id!==id));setSelected(items.find(d=>d.id!==id)?.id??'');setConfirm(false);setCopied('');onUnavailable?.(id);}}catch{if(alive.current)setError(t('textDraft.deleteFailed'));}finally{running.current=false;if(alive.current)setBusy(false);}};
  const finishLeave=(target:'dialog'|'editor')=>{setLeaveTarget(null);if(target==='dialog')onClose();else{setEditing(null);setEditText('');setEditSaved(null);}};
  const requestLeave=(target:'dialog'|'editor')=>{if(running.current||busy)return;if(editing&&editText!==editing.text&&editText!==editSaved)setLeaveTarget(target);else finishLeave(target);};
  const close=()=>requestLeave('dialog');
  return <dialog ref={dialog} className="ol-device-layout-dialog ol-thinscroll" aria-labelledby="saved-text-drafts-title" onCancel={e=>{e.preventDefault();close();}}>
    <div className="ol-device-layout-heading"><h2 id="saved-text-drafts-title">{t('textDraft.title')}</h2><Btn size="sm" variant="ghost" disabled={busy} onClick={close}>{t('common.close')}</Btn></div>
    <p className="ol-device-layout-hint">{t(isTauri?'textWorkspace.draftsIntro':'textDraft.browser')}</p>
    {missingRequested&&<p role="status">{t('textDraft.missingSelected')}</p>}
    {loading?<p role="status">{t('common.loading')}</p>:loadError?<p role="alert">{t('textDraft.loadFailed')} <Btn size="sm" onClick={()=>void refresh()}>{t('common.retry')}</Btn></p>:!items.length?<p>{t('textDraft.empty')}</p>:<>
      <label>{t('textDraft.choose')}<select aria-label={t('textDraft.choose')} value={selectedId} disabled={busy||!!editing} onChange={e=>{setSelected(e.target.value);setMissingRequested(false);setConfirm(false);setError('');setCopied('');}}>
        {missingRequested&&<option value="" disabled>{t('textDraft.choose')}</option>}
        {items.map(d=><option key={d.id} value={d.id}>{draftLabel(d)}</option>)}
      </select></label>
      {editing?<SavedTextDraftCopyEditor record={editing} text={editText} disabled={!!leaveTarget}
        onTextChange={setEditText} onBusy={setBusy}
        onCreated={record=>{setItems(current=>[record,...current.filter(d=>d.id!==record.id)]);setSelected(record.id);setEditSaved(record.text);}}
        onBack={()=>requestLeave('editor')}/>:selected&&<section aria-label={t('textDraft.selected')}>
        <textarea readOnly aria-label={t('textDraft.selected')} value={selected.text} rows={9} style={{width:'100%',lineHeight:1.6,resize:'vertical',marginTop:14}}/>
        <details style={{margin:'14px 0'}}><summary>{t('textDraft.original')}</summary><pre className="ol-saved-phrase-text">{selected.original}</pre></details>
        <div style={{display:'flex',gap:10,flexWrap:'wrap'}}><Btn size="sm" disabled={busy} onClick={()=>void copy()}>{t(busy?'textDraft.working':copied===selected.id?'textDraft.copied':'textDraft.copy')}</Btn>
          <Btn size="sm" variant="ghost" disabled={busy} onClick={()=>{setEditing({...selected});setEditText(selected.text);setEditSaved(null);setConfirm(false);setCopied('');setError('');}}>{t('textDraft.editCopy')}</Btn>
          <Btn size="sm" variant="ghost" disabled={busy} onClick={()=>setConfirm(true)}>{t('textDraft.delete')}</Btn></div>
        {copied===selected.id&&<p role="status">{t('textDraft.paste')}</p>}
        {confirm&&<div role="alert"><p>{t('textDraft.confirm')}</p><Btn size="sm" variant="ghost" disabled={busy} onClick={()=>setConfirm(false)}>{t('common.cancel')}</Btn> <Btn size="sm" disabled={busy} onClick={()=>void remove()}>{t('textDraft.delete')}</Btn></div>}
      </section>}
    </>}
    {leaveTarget&&<div role="alert"><p>{t('textDraft.discardCopyConfirm')}</p>
      <Btn size="sm" variant="ghost" onClick={()=>setLeaveTarget(null)}>{t('common.cancel')}</Btn>{' '}
      <Btn size="sm" onClick={()=>finishLeave(leaveTarget)}>{t('textDraft.discardCopy')}</Btn>
    </div>}
    {error&&<p role="alert">{error}</p>}
    <details className="ol-style-trial-details"><summary>{t('textWorkspace.draftsDetails')}</summary><p className="ol-device-layout-hint">{t('textDraft.intro')}</p></details>
  </dialog>;
}
