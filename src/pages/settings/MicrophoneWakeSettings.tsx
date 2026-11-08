import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getMicrophoneWakeStatus, type MicrophoneWakeStatus } from '../../lib/ipc';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { SettingRow, Toggle, inputStyle } from './shared';

export function MicrophoneWakeSettings() {
  const { t } = useTranslation();
  const { prefs, updatePrefs } = useHotkeySettings();
  const [phrase, setPhrase] = useState(prefs?.voiceWakePhrase ?? '');
  const [status, setStatus] = useState<MicrophoneWakeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => { setPhrase(prefs?.voiceWakePhrase ?? ''); }, [prefs?.voiceWakePhrase]);
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const next = await getMicrophoneWakeStatus();
        if (!cancelled) setStatus(next);
      } catch { if (!cancelled) setStatus(null); }
      if (!cancelled) timer = setTimeout(refresh, 2000);
    };
    void refresh();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [prefs?.microphoneWakeEnabled]);
  if (!prefs) return null;
  const save = async (patch: { microphoneWakeEnabled?: boolean; voiceWakePhrase?: string }) => {
    if (busy) return;
    setBusy(true); setError('');
    try { await updatePrefs(current => ({ ...current, ...patch })); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  };
  const state = prefs.microphoneWakeEnabled ? status?.state ?? 'preparing' : 'off';
  return <>
    <SettingRow label={t('settings.recording.microphoneWakeTitle')} desc={t(prefs.activeAsrProvider === 'volcengine'
      ? 'settings.recording.microphoneWakeAutomaticDesc' : 'settings.recording.microphoneWakeDesc')}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, pointerEvents: busy ? 'none' : undefined, opacity: busy ? 0.6 : 1 }}>
        <span role="status" style={{ fontSize: 11, color: 'var(--ol-ink-3)' }}>{t(`settings.recording.microphoneWakeStates.${state}`)}</span>
        <Toggle on={prefs.microphoneWakeEnabled} onToggle={() => void save({ microphoneWakeEnabled: !prefs.microphoneWakeEnabled })} />
      </div>
    </SettingRow>
    <SettingRow label={t('settings.recording.microphoneWakePhrase')} desc={t('settings.recording.microphoneWakePhraseDesc')}>
      <div style={{ display: 'flex', gap: 6 }}>
        <input aria-label={t('settings.recording.microphoneWakePhrase')} style={{ ...inputStyle, width: 170 }} value={phrase} disabled={busy}
          onChange={event => setPhrase(event.target.value)} onKeyDown={event => {
            if (event.key === 'Enter' && phrase.trim()) void save({ voiceWakePhrase: phrase.trim() });
          }} />
        <button type="button" style={inputStyle} disabled={busy || !phrase.trim() || phrase.trim() === prefs.voiceWakePhrase}
          onClick={() => void save({ voiceWakePhrase: phrase.trim() })}>{t('common.save')}</button>
      </div>
    </SettingRow>
    {(error || state === 'error') && <div role="alert" style={{ fontSize: 12, color: 'var(--ol-err)', margin: '6px 0' }}>
      {error || t('settings.recording.microphoneWakeError')}
    </div>}
  </>;
}
