import { useCallback, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { cancelLocalTranslationPreparation, getLocalTranslationStatus, prepareLocalTranslation } from '../lib/ipc';
import type { LocalTranslationStatus } from '../lib/ipc';
import { localTargetCode, localTranslationErrorKey, localTranslationKey } from '../lib/localTranslation';
import type { LocalTranslationReadiness, LocalTranslationQuality } from '../lib/localTranslation';
import { Btn } from '../pages/_atoms';

/** Explicit optional assets only: mounting or choosing a language never downloads. */
export function LocalTranslationSetup({ source, target, onSourceChange, quality = 'standard', onQualityChange, disabled = false, onReadiness, onBusy, showNotice = true, sourceHint }: {
  source: string;
  target: string;
  onSourceChange: (source: string) => void;
  quality?: LocalTranslationQuality;
  onQualityChange?: (quality: LocalTranslationQuality) => void;
  disabled?: boolean;
  onReadiness?: (state: LocalTranslationReadiness) => void;
  onBusy?: (busy: boolean) => void;
  showNotice?: boolean;
  sourceHint?: ReactNode;
}) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<LocalTranslationStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState('');
  const active = useRef(false);
  const refreshing = useRef(false);
  const request = useRef(false);
  const refresh = useCallback(async () => {
    if (refreshing.current) return;
    refreshing.current = true;
    try {
      const value = await getLocalTranslationStatus(quality);
      if (active.current) setStatus(value);
    } catch {
      if (active.current) setFailure('statusFailed');
    } finally { refreshing.current = false; }
  }, [quality]);
  useEffect(() => {
    active.current = true;
    void refresh();
    return () => { active.current = false; };
  }, [refresh]);
  const busy = pending || status?.preparing === true;
  useEffect(() => {
    onBusy?.(busy);
    return () => { onBusy?.(false); };
  }, [busy, onBusy]);
  useEffect(() => {
    if (!busy) return;
    const timer = window.setInterval(() => void refresh(), 400);
    return () => window.clearInterval(timer);
  }, [busy, refresh]);
  const targetCode = localTargetCode(target);
  const pair = status?.pairs.find(item => item.source === source && item.target === targetCode);
  const ready = !!status?.supported && !!pair?.ready && !busy;
  const key = localTranslationKey(source, target, quality);
  useEffect(() => { onReadiness?.({ key, ready }); }, [key, ready, onReadiness]);

  const prepare = async () => {
    if (disabled || busy || request.current || !pair || !status?.supported) return;
    request.current = true;
    setPending(true); setFailure('');
    try {
      const value = await prepareLocalTranslation(source, targetCode, quality);
      if (active.current) setStatus(value);
    } catch (error) {
      if (active.current) setFailure(localTranslationErrorKey(error));
    } finally {
      request.current = false;
      if (active.current) { setPending(false); void refresh(); }
    }
  };
  const downloadBytes = (pair?.downloadBytes ?? 0) + (status?.runtimeReady ? 0 : status?.runtimeDownloadBytes ?? 0);
  return <div className="ol-local-translation-setup" data-local-translation-setup>
    {onQualityChange && <label className="ol-style-trial-picker">
      <span>{t('offlineTranslation.quality')}</span>
      <select aria-label={t('offlineTranslation.quality')} value={quality} disabled={disabled || busy}
        onChange={event => onQualityChange(event.target.value as LocalTranslationQuality)}>
        <option value="standard">{t('offlineTranslation.standard')}</option>
        <option value="enhanced">{t('offlineTranslation.enhanced')}</option>
      </select>
    </label>}
    {quality === 'enhanced' && <p className="ol-style-trial-hint">{t('offlineTranslation.enhancedHint')}</p>}
    {onQualityChange && <details className="ol-style-trial-details" data-local-model-help>
      <summary>{t('offlineTranslation.modelHelp')}</summary>
      <p className="ol-style-trial-hint">{t('offlineTranslation.modelChoice')}</p>
      {quality !== 'enhanced' && <p className="ol-style-trial-hint">{t('offlineTranslation.enhancedHint')}</p>}
      <p className="ol-style-trial-hint">{t('offlineTranslation.modelPreparation')}</p>
    </details>}
    <label className="ol-style-trial-picker">
      <span>{t('offlineTranslation.source')}</span>
      <select aria-label={t('offlineTranslation.source')} value={source} disabled={disabled || busy}
        onChange={event => { setFailure(''); onSourceChange(event.target.value); }}>
        <option value="">{t('offlineTranslation.chooseSource')}</option>
        <option value="zh">中文</option><option value="en">English</option>
      </select>
    </label>
    {sourceHint}
    {showNotice && <p className="ol-style-trial-hint">{t('offlineTranslation.localNotice')}</p>}
    {!status && !failure && <p role="status">{t('common.loading')}</p>}
    {status && !status.supported && <p role="status">{t('offlineTranslation.platform')}</p>}
    {status?.supported && !source && <p role="status">{t('offlineTranslation.chooseSource')}</p>}
    {status?.supported && source && !pair && <p role="status">{t('offlineTranslation.unsupported')}</p>}
    {ready && <p role="status">{t('offlineTranslation.ready')}</p>}
    {status?.supported && pair && !busy && <Btn size="sm" disabled={disabled} onClick={() => void prepare()}>
      {t(ready ? 'offlineTranslation.repair' : 'offlineTranslation.prepare', { mb: Math.ceil(downloadBytes / 1_000_000) })}
    </Btn>}
    {busy && <div role="status">
      <p>{t('offlineTranslation.downloading', { mb: (status?.receivedBytes ?? 0) / 1_000_000 < 1 ? '0' : Math.floor((status?.receivedBytes ?? 0) / 1_000_000), total: Math.ceil((status?.expectedBytes || downloadBytes) / 1_000_000) })}</p>
      <progress aria-label={t('offlineTranslation.progress')} value={status?.receivedBytes ?? 0} max={status?.expectedBytes || downloadBytes || 1} style={{ width: '100%' }} />
      <Btn size="sm" variant="ghost" onClick={() => {
        void cancelLocalTranslationPreparation().catch(() => { if (active.current) setFailure('failed'); });
      }}>{t('common.cancel')}</Btn>
      <p className="ol-style-trial-hint">{t('offlineTranslation.background')}</p>
    </div>}
    {failure && <p role={failure === 'cancelled' ? 'status' : 'alert'} className="ol-style-trial-hint">
      {t(`offlineTranslation.${failure}`)} <Btn size="sm" variant="ghost" onClick={() => { setFailure(''); void refresh(); }}>{t('common.retry')}</Btn>
    </p>}
  </div>;
}
