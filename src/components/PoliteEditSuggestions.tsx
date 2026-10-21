import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { suggestPoliteTextEdits } from '../lib/ipc';
import { applyLocalPoliteOption, applyPoliteEdits, localPoliteOptions, reviewPoliteEdits, type PoliteEditReview } from '../lib/politeEdits';
import { Btn } from '../pages/_atoms';

export function PoliteEditSuggestions({ text, disabled, onApply, cloudConfigured = true }: {
  text: string; disabled: boolean; onApply: (text: string) => void;
  cloudConfigured?: boolean;
}) {
  const { t } = useTranslation();
  const alive = useRef(false);
  const running = useRef(false);
  const [pending, setPending] = useState(false);
  const [reply, setReply] = useState<{ source: string; review: PoliteEditReview; elapsedMs: number } | null>(null);
  const [selected, setSelected] = useState<number[]>([]);
  const [error, setError] = useState('');
  const [applied, setApplied] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const panel = useRef<HTMLElement>(null);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    if (!open) return;
    // Reveal the choices the user explicitly opened, without moving input focus.
    const frame = requestAnimationFrame(() => panel.current?.scrollIntoView({ block: 'nearest', behavior: 'instant' }));
    return () => cancelAnimationFrame(frame);
  }, [open]);
  const current = reply?.source === text ? reply : null;
  const localOptions = localPoliteOptions(text);
  const request = async () => {
    if (running.current || disabled || !cloudConfigured || !text.trim() || text.length > 4000) return;
    const source = text;
    running.current = true; setPending(true); setError(''); setReply(null); setSelected([]); setApplied(null);
    try {
      const response = await suggestPoliteTextEdits(source);
      const review = reviewPoliteEdits(source, response.proposalJson);
      if (alive.current) setReply({ source, review, elapsedMs: response.elapsedMs });
    } catch (cause) {
      if (alive.current) setError(String(cause).includes('native_required') ? 'native' : 'failed');
    } finally {
      running.current = false;
      if (alive.current) setPending(false);
    }
  };
  const apply = () => {
    if (!current || disabled || pending || !selected.length) return;
    try {
      const updated = applyPoliteEdits(text, selected.map(index => current.review.edits[index]));
      onApply(updated); setApplied(updated); setReply(null); setSelected([]);
    } catch { setError('failed'); }
  };
  return <>
    <Btn size="sm" variant="ghost" disabled={disabled || !text.trim()} onClick={() => setOpen(value => !value)}>{t('politeEdits.open')}</Btn>
    {open && <section ref={panel} aria-label={t('politeEdits.title')} className="ol-style-trial-comparisons" style={{ flexBasis: '100%', minWidth: 0, margin: '4px 0' }}>
    <p className="ol-style-trial-hint">{t('politeEdits.localHint')}</p>
    {localOptions.length === 0 && <p className="ol-style-trial-hint">{t('politeEdits.localNone')}</p>}
    {localOptions.map(edit => <div key={edit.old + '\0' + edit.new} style={{ margin: '10px 0', lineHeight: 1.6, overflowWrap: 'anywhere' }}>
      <strong>{t(`politeEdits.${edit.tone}`)}</strong>
      <p style={{ margin: '4px 0' }}><del>{edit.old}</del>{' → '}<ins>{edit.new}</ins></p>
      <Btn size="sm" disabled={disabled || pending} onClick={() => {
        try {
          const updated = applyLocalPoliteOption(text, edit);
          onApply(updated); setApplied(updated); setReply(null); setSelected([]); setError('');
        } catch { setError('failed'); }
      }}>{t('politeEdits.useLocal')}</Btn>
    </div>)}
    <p className="ol-style-trial-hint">{t(cloudConfigured ? 'politeEdits.cost' : 'politeEdits.cloudSetupHint')}</p>
    <Btn size="sm" variant="ghost" disabled={disabled || pending || !cloudConfigured || !text.trim() || text.length > 4000} onClick={() => void request()}>
      {t(pending ? 'politeEdits.pending' : 'politeEdits.request')}
    </Btn>
    {current && <>
      <p role="status" className="ol-style-trial-hint">{t(current.review.edits.length ? 'politeEdits.review' : 'politeEdits.none', { time: (current.elapsedMs / 1000).toFixed(1) })}</p>
      {current.review.skipped > 0 && <p role="note" className="ol-style-trial-hint">{t('politeEdits.skipped')}</p>}
      {current.review.edits.map((edit, index) => <label key={index} style={{ display: 'block', margin: '10px 0', lineHeight: 1.6 }}>
        <input type="checkbox" checked={selected.includes(index)} disabled={disabled} onChange={event => {
          setSelected(value => event.target.checked ? [...value, index] : value.filter(item => item !== index));
        }} />{' '}<del>{edit.old}</del>{' → '}<ins>{edit.new}</ins>
      </label>)}
      {current.review.edits.length > 0 && <Btn size="sm" disabled={disabled || !selected.length} onClick={apply}>{t('politeEdits.apply')}</Btn>}
      <Btn size="sm" variant="ghost" disabled={disabled} onClick={() => { setReply(null); setSelected([]); }}>{t('politeEdits.keep')}</Btn>
    </>}
    {reply && !current && <p role="status" className="ol-style-trial-hint">{t('politeEdits.stale')}</p>}
    {applied === text && <p role="status" className="ol-style-trial-hint">{t('politeEdits.applied')}</p>}
    {error && <p role="alert" className="ol-style-trial-hint">{t(`politeEdits.${error}`)}</p>}
  </section>}
  </>;
}
