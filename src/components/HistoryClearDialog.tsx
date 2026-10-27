import { useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { Btn } from '../pages/_atoms';

/** An explicit product confirmation, independent of native script dialogs. */
export function HistoryClearDialog({ count, busy, error, onCancel, onConfirm }: {
  count: number; busy: boolean; error: string | null;
  onCancel: () => void; onConfirm: () => void;
}) {
  const { t } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    // Enter after opening must choose Cancel, never confirm a deletion.
    element?.querySelector<HTMLButtonElement>('button')?.focus();
    return () => element?.close();
  }, []);
  return <dialog ref={dialog} className="ol-history-clear-dialog" role="alertdialog"
    aria-labelledby={title} aria-busy={busy}
    onCancel={event => { event.preventDefault(); if (!busy) onCancel(); }}>
    <h2 id={title}>{t('history.confirmClear', { count })}</h2>
    {error && <p role="alert">{error}</p>}
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', marginTop: 22 }}>
      <Btn size="sm" variant="ghost" disabled={busy} onClick={onCancel}>{t('common.cancel')}</Btn>
      <Btn size="sm" disabled={busy || count === 0} onClick={onConfirm}>{t('common.clear')}</Btn>
    </div>
  </dialog>;
}
