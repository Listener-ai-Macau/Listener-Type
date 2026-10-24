import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { suggestPoliteTextEdits } from '../lib/ipc';
import { applyLocalPoliteOption, applyPoliteEdits, localPoliteOptions, reviewPoliteEdits, type PoliteEditReview } from '../lib/politeEdits';
import { Btn } from '../pages/_atoms';

type SuggestionFailure = 'native' | 'requestFailed' | 'unusableReply' | 'applyFailed';
type AppliedWording = { before: string; after: string; label: 'courteous' | 'softer' | 'chosenEdits' };

export function PoliteEditSuggestions({ text, originalText, disabled, onApply, cloudConfigured = true, localOnly = false }: {
  text: string; disabled: boolean; onApply: (text: string) => void;
  originalText?: string;
  cloudConfigured?: boolean;
  localOnly?: boolean;
}) {
  const { t } = useTranslation();
  const alive = useRef(false);
  const running = useRef(false);
  const [pending, setPending] = useState(false);
  const [reply, setReply] = useState<{ source: string; review: PoliteEditReview; elapsedMs: number } | null>(null);
  const [selected, setSelected] = useState<number[]>([]);
  const [error, setError] = useState<{ source: string; kind: SuggestionFailure } | null>(null);
  const [applied, setApplied] = useState<AppliedWording | null>(null);
  const [open, setOpen] = useState(false);
  const panel = useRef<HTMLElement>(null);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  // A hand edit supersedes the receipt even if the user later types its old
  // output again. A fresh translation also remounts this result-owned review.
  useEffect(() => { setApplied(value => value && value.after !== text ? null : value); }, [text]);
  useEffect(() => {
    if (!open) return;
    // Reveal the choices the user explicitly opened, without moving input focus.
    const frame = requestAnimationFrame(() => panel.current?.scrollIntoView({ block: 'nearest', behavior: 'instant' }));
    return () => cancelAnimationFrame(frame);
  }, [open]);
  const current = !localOnly && reply?.source === text ? reply : null;
  const currentApplied = applied?.after === text ? applied : null;
  const localOptions = localPoliteOptions(text);
  const request = async () => {
    if (running.current || disabled || localOnly || !cloudConfigured || !text.trim() || text.length > 4000) return;
    const source = text;
    running.current = true; setPending(true); setError(null); setReply(null); setSelected([]);
    let receivedReply = false;
    try {
      const response = await suggestPoliteTextEdits(source);
      receivedReply = true;
      const review = reviewPoliteEdits(source, response.proposalJson);
      if (alive.current) setReply({ source, review, elapsedMs: response.elapsedMs });
    } catch (cause) {
      // A delivered but unusable proposal is not evidence of broken credentials.
      // The native endpoint can also reject an empty/oversized service response.
      const message = String(cause);
      const kind: SuggestionFailure = receivedReply || message.includes('polite_edits_invalid_response')
        ? 'unusableReply' : message.includes('native_required') ? 'native' : 'requestFailed';
      if (alive.current) setError({ source, kind });
    } finally {
      running.current = false;
      if (alive.current) setPending(false);
    }
  };
  const apply = () => {
    if (!current || disabled || pending || !selected.length) return;
    try {
      const updated = applyPoliteEdits(text, selected.map(index => current.review.edits[index]));
      onApply(updated); setApplied({ before: text, after: updated, label: 'chosenEdits' }); setReply(null); setSelected([]);
    } catch { setError({ source: text, kind: 'applyFailed' }); }
  };
  const undo = () => {
    if (!currentApplied || disabled || pending) return;
    onApply(currentApplied.before);
    setApplied(null); setReply(null); setSelected([]); setError(null);
  };
  return <>
    <Btn size="sm" variant="ghost" disabled={disabled || !text.trim()} onClick={() => setOpen(value => !value)}>{t('politeEdits.open')}</Btn>
    {open && <section ref={panel} aria-label={t('politeEdits.title')} className="ol-style-trial-comparisons" style={{ flexBasis: '100%', minWidth: 0, margin: '4px 0' }}>
    {originalText !== undefined && <div className="ol-wording-context" data-wording-context>
      <div><h4>{t('style.trial.input')}</h4><p className="ol-wording-context-text ol-thinscroll" data-wording-original>{originalText}</p></div>
      <div><h4>{t('style.trial.output')}</h4><p className="ol-wording-context-text ol-thinscroll" data-wording-current>{text}</p></div>
    </div>}
    <div className="ol-wording-choices ol-thinscroll" data-wording-choices>
    <p className="ol-style-trial-hint">{t(localOnly ? 'politeEdits.localOnlyHint' : 'politeEdits.localHint')}</p>
    {currentApplied && <div role="status" data-wording-applied style={{ margin: '10px 0' }}>
      <strong>{t('politeEdits.appliedChoice', { choice: t(`politeEdits.${currentApplied.label}`) })}</strong>{' '}
      <Btn size="sm" variant="ghost" disabled={disabled || pending} onClick={undo}>{t('politeEdits.undo')}</Btn>
      <p className="ol-style-trial-hint">{t('politeEdits.undoHint')}</p>
    </div>}
    {localOptions.length === 0 && !currentApplied && <p className="ol-style-trial-hint">{t(localOnly ? 'politeEdits.localOnlyNone' : 'politeEdits.localNone')}</p>}
    {localOptions.map(edit => <div key={edit.old + '\0' + edit.new} style={{ margin: '10px 0', lineHeight: 1.6, overflowWrap: 'anywhere' }}>
      <strong>{t(`politeEdits.${edit.tone}`)}</strong>
      <p className="ol-wording-candidate ol-thinscroll" data-wording-preview>
        {text.slice(0, text.indexOf(edit.old))}<ins>{edit.new}</ins>{text.slice(text.indexOf(edit.old) + edit.old.length)}
      </p>
      <Btn size="sm" disabled={disabled || pending} onClick={() => {
        try {
          const updated = applyLocalPoliteOption(text, edit);
          onApply(updated); setApplied({ before: text, after: updated, label: edit.tone }); setReply(null); setSelected([]); setError(null);
        } catch { setError({ source: text, kind: 'applyFailed' }); }
      }}>{t('politeEdits.useLocal')}</Btn>
    </div>)}
    {!localOnly && <><p className="ol-style-trial-hint">{t(cloudConfigured ? 'politeEdits.cost' : 'politeEdits.cloudSetupHint')}</p>
    <Btn size="sm" variant="ghost" disabled={disabled || pending || !cloudConfigured || !text.trim() || text.length > 4000} onClick={() => void request()}>
      {t(pending ? 'politeEdits.pending' : 'politeEdits.request')}
    </Btn></>}
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
    {!localOnly && reply && !current && <p role="status" className="ol-style-trial-hint">{t('politeEdits.stale')}</p>}
    {error?.source === text && (!localOnly || error.kind === 'applyFailed') && <p role="alert" className="ol-style-trial-hint">{t(`politeEdits.${error.kind}`)}</p>}
    </div>
  </section>}
  </>;
}
