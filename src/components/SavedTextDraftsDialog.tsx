import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { copyExplicitText, deleteSavedTextDraft, isTauri, listSavedTextDrafts, type SavedTextDraft } from '../lib/ipc';
import { detectOS } from './WindowChrome';
import { Btn } from '../pages/_atoms';
import './DeviceLayoutsDialog.css';

export function SavedTextDraftsDialog({onClose}:{onClose:()=>void}) {
  const {t}=useTranslation();const dialog=useRef<HTMLDialogElement>(null),alive=useRef(true),running=useRef(false);
  const [items,setItems]=useState<SavedTextDraft[]>([]),[selectedId,setSelected]=useState('');
  const [loading,setLoading]=useState(true),[busy,setBusy]=useState(false),[loadError,setLoadError]=useState(false),[error,setError]=useState(''),[copied,setCopied]=useState(''),[confirm,setConfirm]=useState(false);
  const selected=items.find(d=>d.id===selectedId);
  const refresh=async()=>{setLoading(true);setLoadError(false);try{const value=await listSavedTextDrafts();if(alive.current){setItems(value);setSelected(current=>value.some(d=>d.id===current)?current:value[0]?.id??'');}}catch{if(alive.current)setLoadError(true);}finally{if(alive.current)setLoading(false);}};
  useEffect(()=>{alive.current=true;dialog.current?.showModal();void refresh();return()=>{alive.current=false;dialog.current?.close();};},[]);
  const copy=async()=>{if(!selected||running.current)return;const value=selected;running.current=true;setBusy(true);setError('');setCopied('');try{await copyExplicitText(value.text,detectOS()==='win');if(alive.current)setCopied(value.id);}catch{if(alive.current)setError(t('textDraft.copyFailed'));}finally{running.current=false;if(alive.current)setBusy(false);}};
  const remove=async()=>{if(!selected||running.current)return;const id=selected.id;running.current=true;setBusy(true);setError('');try{await deleteSavedTextDraft(id);if(alive.current){setItems(current=>current.filter(d=>d.id!==id));setSelected(items.find(d=>d.id!==id)?.id??'');setConfirm(false);setCopied('');}}catch{if(alive.current)setError(t('textDraft.deleteFailed'));}finally{running.current=false;if(alive.current)setBusy(false);}};
  const close=()=>{if(!running.current)onClose();};
  return <dialog ref={dialog} className="ol-device-layout-dialog ol-thinscroll" aria-labelledby="saved-text-drafts-title" onCancel={e=>{e.preventDefault();close();}}>
    <div className="ol-device-layout-heading"><h2 id="saved-text-drafts-title">{t('textDraft.title')}</h2><Btn size="sm" variant="ghost" disabled={busy} onClick={close}>{t('common.close')}</Btn></div>
    <p className="ol-device-layout-hint">{t('textDraft.intro')}</p>
    {!isTauri&&<p role="note" className="ol-device-layout-hint">{t('textDraft.browser')}</p>}
    {loading?<p role="status">{t('common.loading')}</p>:loadError?<p role="alert">{t('textDraft.loadFailed')} <Btn size="sm" onClick={()=>void refresh()}>{t('common.retry')}</Btn></p>:!items.length?<p>{t('textDraft.empty')}</p>:<>
      <label>{t('textDraft.choose')}<select aria-label={t('textDraft.choose')} value={selectedId} disabled={busy} onChange={e=>{setSelected(e.target.value);setConfirm(false);setError('');setCopied('');}}>
        {items.map(d=><option key={d.id} value={d.id}>{d.name} · {new Date(d.createdAt).toLocaleString()} · {Array.from(d.text.replace(/\s+/g,' ')).slice(0,55).join('')}</option>)}
      </select></label>
      {selected&&<section aria-label={t('textDraft.selected')}>
        <p className="ol-device-layout-hint">{selected.name} · {new Date(selected.createdAt).toLocaleString()}</p>
        <textarea readOnly aria-label={t('textDraft.selected')} value={selected.text} rows={9} style={{width:'100%',lineHeight:1.6,resize:'vertical'}}/>
        <details style={{margin:'14px 0'}}><summary>{t('textDraft.original')}</summary><pre className="ol-saved-phrase-text">{selected.original}</pre></details>
        <div style={{display:'flex',gap:10,flexWrap:'wrap'}}><Btn size="sm" disabled={busy} onClick={()=>void copy()}>{t(busy?'textDraft.working':copied===selected.id?'textDraft.copied':'textDraft.copy')}</Btn><Btn size="sm" variant="ghost" disabled={busy} onClick={()=>setConfirm(true)}>{t('textDraft.delete')}</Btn></div>
        {copied===selected.id&&<p role="status">{t('textDraft.paste')}</p>}
        {confirm&&<div role="alert"><p>{t('textDraft.confirm')}</p><Btn size="sm" variant="ghost" disabled={busy} onClick={()=>setConfirm(false)}>{t('common.cancel')}</Btn> <Btn size="sm" disabled={busy} onClick={()=>void remove()}>{t('textDraft.delete')}</Btn></div>}
      </section>}
    </>}
    {error&&<p role="alert">{error}</p>}
  </dialog>;
}
