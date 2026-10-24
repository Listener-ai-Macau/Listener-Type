import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getPendingEmbeddedBleRecovery, openSystemSettings, recoverEmbeddedBleDevice } from '../lib/ipc';
import { embeddedBleProbeErrorMessage } from '../lib/embeddedBleProbe';
import type { EmbeddedBleRepairResult } from '../lib/types';

export function DeviceRecoveryControl() {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(() => !!getPendingEmbeddedBleRecovery());
  const [result, setResult] = useState<EmbeddedBleRepairResult | null>(null);
  const [error, setError] = useState<{ kind: 'recovery' | 'system'; cause: unknown } | null>(null);
  const pending = useRef(busy);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    // Reopening About observes the outstanding operation. It must not cancel
    // a fresh dictation or open Windows Settings a second time.
    const request = getPendingEmbeddedBleRecovery();
    let observing = true;
    if (request) {
      pending.current = true;
      setBusy(true);
      void request.then(value => {
        if (observing) { setResult(value); setError(null); }
      }, cause => {
        if (observing) setError({ kind: 'recovery', cause });
      }).finally(() => {
        if (observing) { pending.current = false; setBusy(false); }
      });
    }
    return () => { observing = false; mounted.current = false; };
  }, []);

  const openBluetooth = async () => {
    if (!mounted.current) return;
    try { await openSystemSettings('bluetooth'); }
    catch (cause) { if (mounted.current) setError({ kind: 'system', cause }); }
  };

  const repair = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true); setResult(null); setError(null);
    try {
      const request = getPendingEmbeddedBleRecovery() ?? recoverEmbeddedBleDevice(15_000, true);
      const value = await request;
      if (!mounted.current) return;
      setResult(value);
      if (value.openBluetoothSettings) await openBluetooth();
    } catch (cause) {
      if (!mounted.current) return;
      setError({ kind: 'recovery', cause });
      // Retain the existing fallback, with a visible failure if it also fails.
      await openBluetooth();
    } finally {
      if (mounted.current) { pending.current = false; setBusy(false); }
    }
  };

  const message = busy
    ? t('settings.recording.embeddedBleConnectionMessageChecking')
    : error
      ? error.kind === 'system' ? t('modal.about.bluetoothSettingsOpenFailed') : embeddedBleProbeErrorMessage(error.cause, t)
      : result?.recovered
        ? t('settings.recording.embeddedBleConnectionReady')
        : result?.message || (result ? t('settings.recording.embeddedBleConnectionError') : '');
  return <div data-device-recovery-control style={{ display: 'grid', gap: 7 }}>
    <button disabled={busy} onClick={() => void repair()} style={{
      padding: '5px 10px', border: '0.5px solid var(--ol-line-strong)',
      borderRadius: 6, background: 'transparent', color: 'var(--ol-ink-2)',
      font: 'inherit', fontSize: 12, justifySelf: 'start', cursor: busy ? 'wait' : 'pointer', opacity: busy ? .65 : 1,
    }}>{t(busy ? 'settings.recording.embeddedBleConnectionChecking' : 'modal.about.openBluetoothSettings')}</button>
    {message && <span role="status" aria-live="polite" data-device-recovery-status style={{
      maxWidth: 290, fontSize: 11, lineHeight: 1.6, overflowWrap: 'anywhere',
      color: error || (result && !result.recovered) ? 'var(--ol-err)' : 'var(--ol-ink-3)',
    }}>{message}</span>}
  </div>;
}
