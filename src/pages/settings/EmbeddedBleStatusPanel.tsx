import { useTranslation } from 'react-i18next';
import { Btn, Pill } from '../_atoms';
import { requestDemoMode } from '../../lib/demoMode';
import type { EmbeddedAudioSubmissionResult } from '../../lib/types';

export type EmbeddedBleProbeStatus = 'idle' | 'checking' | 'ok' | 'error';
export type EmbeddedBleWizardStepId = 'select' | 'pair' | 'connect' | 'subscribe' | 'record';
export type EmbeddedBleWizardStepState = 'pending' | 'active' | 'ok' | 'error';

export interface EmbeddedBleWizardStep {
  id: EmbeddedBleWizardStepId;
  state: EmbeddedBleWizardStepState;
}

export function EmbeddedBleStatusPanel({
  supported,
  status,
  message,
  result,
  steps,
  onOpenBluetoothSettings,
  onProbe,
  onTestRecording,
  onUseMicrophone,
}: {
  supported: boolean;
  status: EmbeddedBleProbeStatus;
  message: string;
  result: EmbeddedAudioSubmissionResult | null;
  steps: EmbeddedBleWizardStep[];
  onOpenBluetoothSettings: () => void;
  onProbe: () => void;
  onTestRecording?: () => void;
  onUseMicrophone?: () => void;
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
  const stateGuideItems = [
    ['ready', t('settings.recording.embeddedBleStateReady')],
    ['recording', t('settings.recording.embeddedBleStateRecording')],
    ['transferring', t('settings.recording.embeddedBleStateTransferring')],
    ['recovery', t('settings.recording.embeddedBleStateRecovery')],
    ['error', t('settings.recording.embeddedBleStateError')],
  ] as const;

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
            : t('settings.recording.embeddedBleCheck')}
        </Btn>
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
        {supported ? t('settings.recording.embeddedBleWizardDesc') : t('settings.recording.embeddedBleUnsupportedDesc')}
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(5, minmax(0, 1fr))',
          gap: 6,
        }}
      >
        {steps.map((step, index) => (
          <WizardStepPill
            key={step.id}
            index={index + 1}
            label={t(`settings.recording.embeddedBleWizard.${step.id}`)}
            state={!supported ? 'pending' : step.state}
          />
        ))}
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
        <div style={{ fontSize: 11, fontWeight: 600, color: 'var(--ol-ink-3)' }}>
          {t('settings.recording.embeddedBleStateGuideTitle')}
        </div>
        <div style={{ display: 'flex', gap: 5, flexWrap: 'wrap' }}>
          {stateGuideItems.map(([id, label]) => (
            <Pill key={id} tone={id === 'error' ? 'outline' : id === 'ready' ? 'ok' : 'blue'} size="sm">
              {label}
            </Pill>
          ))}
        </div>
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
      {supported && (
        <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
          <Btn variant="ghost" size="sm" icon="settings" onClick={onOpenBluetoothSettings}>
            {t('settings.recording.embeddedBleOpenBluetooth')}
          </Btn>
          {onTestRecording && (
            <Btn
              variant={status === 'ok' ? 'blue' : 'soft'}
              size="sm"
              icon="mic"
              disabled={status === 'checking'}
              onClick={onTestRecording}
            >
              {t('settings.recording.embeddedBleTestOnce')}
            </Btn>
          )}
        </div>
      )}
      {supported && status === 'error' && onUseMicrophone && (
        <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
          <Btn variant="ghost" size="sm" icon="refresh" onClick={onProbe}>
            {t('settings.recording.embeddedBleRetry')}
          </Btn>
          <Btn variant="soft" size="sm" icon="mic" onClick={onUseMicrophone}>
            {t('settings.recording.embeddedBleUseMicrophone')}
          </Btn>
          <Btn variant="soft" size="sm" icon="sparkle" onClick={requestDemoMode}>
            {t('settings.recording.embeddedBleOpenDemo')}
          </Btn>
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

function WizardStepPill({
  index,
  label,
  state,
}: {
  index: number;
  label: string;
  state: EmbeddedBleWizardStepState;
}) {
  const color =
    state === 'ok'
      ? 'var(--ol-ok)'
      : state === 'error'
        ? 'var(--ol-err)'
        : state === 'active'
          ? 'var(--ol-blue)'
          : 'var(--ol-ink-4)';
  const background =
    state === 'ok'
      ? 'var(--ol-ok-soft)'
      : state === 'active'
        ? 'var(--ol-blue-soft)'
        : state === 'error'
          ? 'rgba(190,18,60,0.08)'
          : 'rgba(0,0,0,0.035)';
  const marker = state === 'ok' ? '✓' : state === 'error' ? '!' : index;

  return (
    <div
      style={{
        minWidth: 0,
        display: 'flex',
        alignItems: 'center',
        gap: 5,
        height: 26,
        padding: '0 7px',
        borderRadius: 7,
        background,
        color,
        fontSize: 10.5,
        fontWeight: 500,
      }}
      title={label}
    >
      <span
        style={{
          width: 14,
          height: 14,
          borderRadius: 999,
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          flexShrink: 0,
          background: 'rgba(255,255,255,0.68)',
          fontSize: 9,
          fontWeight: 700,
        }}
      >
        {marker}
      </span>
      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        {label}
      </span>
    </div>
  );
}
