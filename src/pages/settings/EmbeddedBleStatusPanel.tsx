import { useTranslation } from 'react-i18next';
import { Btn, Pill } from '../_atoms';
import type { EmbeddedAudioSubmissionResult } from '../../lib/types';

export type EmbeddedBleProbeStatus = 'idle' | 'checking' | 'ok' | 'error';

export function EmbeddedBleStatusPanel({
  supported,
  status,
  message,
  result,
  onProbe,
}: {
  supported: boolean;
  status: EmbeddedBleProbeStatus;
  message: string;
  result: EmbeddedAudioSubmissionResult | null;
  onProbe: () => void;
}) {
  const { t } = useTranslation();
  const pillTone: 'outline' | 'ok' | 'blue' = !supported ? 'outline' : status === 'ok' ? 'ok' : status === 'checking' ? 'blue' : 'outline';
  const statusLabel = !supported
    ? t('settings.recording.embeddedBleUnsupported')
    : status === 'checking'
      ? t('settings.recording.embeddedBleChecking')
      : status === 'ok'
        ? t('settings.recording.embeddedBleReady')
        : status === 'error'
          ? t('settings.recording.embeddedBleError')
          : t('settings.recording.embeddedBleIdle');
  const stats = result?.stats;

  return (
    <div
      style={{
        padding: '10px 12px',
        borderRadius: 8,
        border: '0.5px solid var(--ol-line-soft)',
        background: 'rgba(0,0,0,0.025)',
        display: 'flex',
        flexDirection: 'column',
        gap: 8,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
        <Pill tone={pillTone} size="sm">{statusLabel}</Pill>
        <Btn
          variant="ghost"
          size="sm"
          icon="refresh"
          disabled={!supported || status === 'checking'}
          onClick={onProbe}
        >
          {status === 'checking'
            ? t('settings.recording.embeddedBleTesting')
            : t('settings.recording.embeddedBleTestOnce')}
        </Btn>
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
        {supported ? t('settings.recording.embeddedBleStatusDesc') : t('settings.recording.embeddedBleUnsupportedDesc')}
      </div>
      {message && (
        <div
          style={{
            fontSize: 11.5,
            color: status === 'error' ? 'var(--ol-err)' : 'var(--ol-ok)',
            lineHeight: 1.5,
            overflowWrap: 'anywhere',
          }}
          title={message}
        >
          {message}
        </div>
      )}
      {stats && (
        <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
          <Pill tone="outline" size="sm">
            {t('settings.recording.embeddedBlePackets', {
              received: stats.receivedPacketCount,
              expected: stats.expectedPacketCount ?? stats.receivedPacketCount,
            })}
          </Pill>
          <Pill tone={stats.missingPacketCount === 0 ? 'ok' : 'outline'} size="sm">
            {t('settings.recording.embeddedBleMissing', { count: stats.missingPacketCount })}
          </Pill>
          <Pill tone="outline" size="sm">
            {t('settings.recording.embeddedBleDuration', { seconds: stats.durationSeconds.toFixed(1) })}
          </Pill>
        </div>
      )}
    </div>
  );
}
