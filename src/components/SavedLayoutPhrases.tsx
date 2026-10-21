import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { copyExplicitText } from '../lib/ipc';
import { layoutGestures, layoutKeyIds } from '../lib/deviceKeyLayouts';
import type { DeviceLayout } from '../lib/types';
import { detectOS } from './WindowChrome';
import { Btn } from '../pages/_atoms';

/** Everyday reuse of actual saved text; the practice buffer remains separate. */
export function SavedLayoutPhrases({ layouts, selectedId, loading, error, onSelect, onBusy }: {
  layouts: DeviceLayout[]; selectedId: string | null; loading: boolean; error: string;
  onSelect: (layout?: DeviceLayout) => void; onBusy: (busy: boolean) => void;
}) {
  const { t } = useTranslation();
  const [copying, setCopying] = useState(false);
  const [copied, setCopied] = useState('');
  const [failed, setFailed] = useState(false);
  const running = useRef(false);
  const alive = useRef(true);
  const saved = layouts.find(layout => layout.id === selectedId);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { setCopied(''); setFailed(false); }, [saved?.id, saved?.revision]);
  const phrases = saved && !loading && !error ? layoutGestures.flatMap((group, gesture) => layoutKeyIds.flatMap(key => {
    if (key === 'knob' && gesture !== 0) return [];
    const mapping = saved.profile[group][key];
    if (mapping.action !== 'pasteTemplate' || !mapping.pasteTemplate.trim()) return [];
    return [{ id:`${group}.${key}`, text:mapping.pasteTemplate,
      key:key === 'knob' ? t('settings.deviceKeys.knob.title') : t('deviceLayouts.key', {number:key.slice(3)}),
      gesture:t(`settings.deviceKeys.gestures.${['singleClick','doubleClick','longPress'][gesture]}`) }];
  })) : [];
  const copy = async (id: string, text: string) => {
    if (running.current) return;
    running.current = true; setCopying(true); setCopied(''); setFailed(false); onBusy(true);
    try { await copyExplicitText(text, detectOS() === 'win'); if (alive.current) setCopied(id); }
    catch { if (alive.current) setFailed(true); }
    finally { running.current = false; if (alive.current) { setCopying(false); onBusy(false); } }
  };
  return <section className="ol-saved-layout-phrases" aria-label={t('savedPhrases.title')}>
    <label>{t('deviceLayouts.library')}<select aria-label={t('deviceLayouts.library')} value={saved?.id ?? ''} disabled={loading || copying || !!error} onChange={event => onSelect(layouts.find(layout => layout.id === event.target.value))}>
      <option value="">{t('savedPhrases.choose')}</option>{layouts.map(layout => <option key={layout.id} value={layout.id}>{layout.name}</option>)}
    </select></label>
    {loading ? <p role="status">{t('common.loading')}</p> : error ? <p role="alert">{error}</p> : !phrases.length && <p className="ol-device-layout-hint">{t('savedPhrases.empty')}</p>}
    {phrases.map(phrase => <div key={phrase.id} className="ol-device-layout-editor">
      <h3>{phrase.key} · {phrase.gesture}</h3>
      <p className="ol-saved-phrase-text">{phrase.text}</p>
      <Btn size="sm" disabled={copying} onClick={() => void copy(phrase.id, phrase.text)}>{t(copying ? 'savedPhrases.copying' : copied === phrase.id ? 'savedPhrases.copied' : 'savedPhrases.copy', {key:phrase.key,gesture:phrase.gesture})}</Btn>
    </div>)}
    {copied && <p role="status">{t('savedPhrases.paste')}</p>}
    {failed && <p role="alert">{t('savedPhrases.failed')}</p>}
  </section>;
}
