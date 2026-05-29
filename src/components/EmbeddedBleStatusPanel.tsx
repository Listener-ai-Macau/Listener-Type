import { useTranslation } from 'react-i18next';
import type { EmbeddedBleProbeStatus } from '../lib/embeddedBleProbe';
import type { BleRecoveryUiModel } from '../lib/bleRecoveryUi';
import { Btn, Pill } from '../pages/_atoms';

export type { EmbeddedBleProbeStatus };

type DiagnosticExportStatus = 'idle' | 'busy' | 'ok' | 'err';

export function EmbeddedBleStatusPanel({
  recovery,
  onOpenBluetoothSettings,
  onProbe,
  onRepair,
  onOpenRecordingSettings,
  onUseMicrophone,
  onExportDiagnostics,
  diagnosticStatus = 'idle',
}: {
  recovery: BleRecoveryUiModel;
  onOpenBluetoothSettings: () => void;
  onProbe: () => void;
  onRepair?: () => void;
  onOpenRecordingSettings?: () => void;
  onUseMicrophone?: () => void;
  onExportDiagnostics?: () => void;
  diagnosticStatus?: DiagnosticExportStatus;
}) {
  const { t } = useTranslation();
  const checking = recovery.state === 'checking';

  return (
    <div
      style={{
        marginTop: 8,
        padding: '14px 16px',
        borderRadius: 8,
        border: '0.5px solid var(--ol-line)',
        background: 'var(--ol-control-track)',
        display: 'flex',
        flexDirection: 'column',
        gap: 12,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('settings.recording.embeddedBleStatusTitle', 'Listener BLE')}
          </div>
          <Pill tone={recovery.tone} size="sm">{recovery.label}</Pill>
        </div>
        <Btn
          variant="ghost"
          size="sm"
          icon="refresh"
          disabled={recovery.state === 'unsupported' || checking}
          onClick={onProbe}
        >
          {t('common.refresh')}
        </Btn>
      </div>
      {recovery.showDetails && (
        <>
          <div style={{ fontSize: 11.5, color: recovery.tone === 'err' ? 'var(--ol-err)' : 'var(--ol-ink-4)', lineHeight: 1.55 }}>
            {recovery.message}
          </div>
          {recovery.emphasizeRePair && (
            <div style={{ fontSize: 11.5, color: 'var(--ol-ink-3)', lineHeight: 1.5 }}>
              {t(
                'settings.recording.embeddedBleRecovery.rePairSteps',
                'In Windows Bluetooth, remove the Listener device, pair it again, then return here and refresh.',
              )}
            </div>
          )}
          <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
            {recovery.showOpenBluetoothSettings && (
              <Btn variant="ghost" size="sm" icon="settings" onClick={onOpenBluetoothSettings}>
                {t('settings.recording.embeddedBleOpenBluetooth')}
              </Btn>
            )}
            {onOpenRecordingSettings && (
              <Btn variant="ghost" size="sm" icon="mic" onClick={onOpenRecordingSettings}>
                {t('overview.deviceHealth.openRecording')}
              </Btn>
            )}
            {recovery.showUseMicrophone && onUseMicrophone && (
              <Btn variant="soft" size="sm" icon="mic" onClick={onUseMicrophone}>
                {t('settings.recording.embeddedBleUseMicrophone')}
              </Btn>
            )}
            {recovery.showRepair && onRepair && (
              <Btn variant="soft" size="sm" icon="refresh" disabled={checking} onClick={onRepair}>
                {t('settings.recording.embeddedBleRepair')}
              </Btn>
            )}
            {recovery.showExportDiagnostics && onExportDiagnostics && (
              <Btn
                variant="soft"
                size="sm"
                icon="doc"
                disabled={diagnosticStatus === 'busy'}
                onClick={onExportDiagnostics}
              >
                {diagnosticStatus === 'busy'
                  ? t('modal.about.exporting')
                  : t('modal.about.exportDiagnosticPackageBtn')}
              </Btn>
            )}
            {diagnosticStatus === 'ok' && (
              <span style={{ fontSize: 11, color: 'var(--ol-ok)' }}>
                {t('modal.about.exportSuccess')}
              </span>
            )}
            {diagnosticStatus === 'err' && (
              <span style={{ fontSize: 11, color: 'var(--ol-err)' }}>
                {t('modal.about.exportFailed')}
              </span>
            )}
          </div>
        </>
      )}
    </div>
  );
}
