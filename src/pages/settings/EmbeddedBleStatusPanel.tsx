import { useTranslation } from 'react-i18next';
import { Btn, Pill } from '../_atoms';

export type EmbeddedBleProbeStatus = 'idle' | 'checking' | 'ok' | 'error';

type ConnectionState = 'idle' | 'checking' | 'ok' | 'error' | 'unsupported';

export function EmbeddedBleStatusPanel({
  supported,
  status,
  message,
  onOpenBluetoothSettings,
  onProbe,
  onUseMicrophone,
}: {
  supported: boolean;
  status: EmbeddedBleProbeStatus;
  message: string;
  onOpenBluetoothSettings: () => void;
  onProbe: () => void;
  onUseMicrophone?: () => void;
}) {
  const { t } = useTranslation();
  const checking = status === 'checking';
  const connectionState: ConnectionState = !supported
    ? 'unsupported'
    : checking
      ? 'checking'
      : status === 'ok'
        ? 'ok'
        : status === 'error'
          ? 'error'
          : 'idle';
  const overallTone: 'outline' | 'ok' | 'blue' | 'err' =
    connectionState === 'ok'
      ? 'ok'
      : connectionState === 'checking'
        ? 'blue'
        : connectionState === 'error'
          ? 'err'
          : 'outline';
  const overallLabel = connectionState === 'ok'
    ? t('settings.recording.embeddedBleHealthShort', '健康')
    : connectionState === 'checking'
      ? t('settings.recording.embeddedBleChecking')
    : connectionState === 'error'
        ? t('settings.recording.embeddedBleErrorShort', '异常')
        : connectionState === 'unsupported'
          ? t('settings.recording.embeddedBleUnsupported')
          : t('settings.recording.embeddedBleDisconnectedShort', '未连接');
  const bodyMessage = message
    || (supported
      ? t('settings.recording.embeddedBleSimpleDesc', '刷新设备状态，确认设备健康能用。')
      : t('settings.recording.embeddedBleUnsupportedDesc'));
  const showDetails = connectionState !== 'ok';

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
          <Pill tone={overallTone} size="sm">{overallLabel}</Pill>
        </div>
        <Btn
          variant="ghost"
          size="sm"
          icon="refresh"
          disabled={!supported || checking}
          onClick={onProbe}
        >
          {t('common.refresh')}
        </Btn>
      </div>
      {showDetails && (
        <>
          <div style={{ fontSize: 11.5, color: status === 'error' ? 'var(--ol-err)' : 'var(--ol-ink-4)', lineHeight: 1.55 }}>
            {bodyMessage}
          </div>
          <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
            <Btn variant="ghost" size="sm" icon="settings" onClick={onOpenBluetoothSettings}>
              {t('settings.recording.embeddedBleOpenBluetooth')}
            </Btn>
            {supported && status === 'error' && onUseMicrophone && (
              <Btn variant="soft" size="sm" icon="mic" onClick={onUseMicrophone}>
                {t('settings.recording.embeddedBleUseMicrophone')}
              </Btn>
            )}
          </div>
        </>
      )}
    </div>
  );
}
