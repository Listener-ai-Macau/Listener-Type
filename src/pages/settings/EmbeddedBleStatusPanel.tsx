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
  const overallTone: 'outline' | 'ok' | 'blue' =
    connectionState === 'ok'
      ? 'ok'
      : connectionState === 'checking'
        ? 'blue'
        : 'outline';
  const overallLabel = connectionState === 'ok'
    ? t('settings.recording.embeddedBleDeviceNormal', '设备健康能用')
    : connectionState === 'checking'
      ? t('settings.recording.embeddedBleChecking')
      : connectionState === 'error'
        ? t('settings.recording.embeddedBleConnectionError', '连接异常')
        : connectionState === 'unsupported'
          ? t('settings.recording.embeddedBleUnsupported')
          : t('settings.recording.embeddedBleIdle');
  const bodyMessage = message
    || (supported
      ? t('settings.recording.embeddedBleSimpleDesc', '刷新设备状态，确认设备健康能用。')
      : t('settings.recording.embeddedBleUnsupportedDesc'));

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
      <div style={{ fontSize: 11.5, color: status === 'error' ? 'var(--ol-err)' : 'var(--ol-ink-4)', lineHeight: 1.55 }}>
        {bodyMessage}
      </div>
      <StatusTile
        label={t('settings.recording.embeddedBleConnectionLabel', '设备状态')}
        state={connectionState}
        idleText={t('settings.recording.embeddedBleConnectionIdle', '未检查')}
        okText={t('settings.recording.embeddedBleDeviceNormal', '设备健康能用')}
        checkingText={t('settings.recording.embeddedBleConnectionChecking', '检查中')}
        errorText={t('settings.recording.embeddedBleConnectionError', '连接异常')}
        unsupportedText={t('settings.recording.embeddedBleUnsupported')}
        actionLabel={t('common.refresh')}
        disabled={!supported || checking}
        onAction={onProbe}
      />
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
    </div>
  );
}

function StatusTile({
  label,
  state,
  idleText,
  okText,
  checkingText,
  errorText,
  unsupportedText,
  actionLabel,
  disabled,
  onAction,
}: {
  label: string;
  state: ConnectionState;
  idleText: string;
  okText: string;
  checkingText: string;
  errorText: string;
  unsupportedText: string;
  actionLabel: string;
  disabled: boolean;
  onAction?: () => void;
}) {
  const value = state === 'ok'
    ? okText
    : state === 'checking'
      ? checkingText
      : state === 'error'
        ? errorText
        : state === 'unsupported'
          ? unsupportedText
          : idleText;
  const color = state === 'ok'
    ? 'var(--ol-ok)'
    : state === 'error'
      ? 'var(--ol-err)'
      : state === 'checking'
        ? 'var(--ol-blue)'
        : 'var(--ol-ink-3)';

  return (
    <div
      style={{
        minWidth: 0,
        minHeight: 74,
        padding: '10px 12px',
        borderRadius: 8,
        border: '0.5px solid var(--ol-line-soft)',
        background: 'var(--ol-surface)',
        display: 'grid',
        gridTemplateColumns: 'minmax(0, 1fr) auto',
        alignItems: 'center',
        gap: 10,
      }}
    >
      <div style={{ minWidth: 0 }}>
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginBottom: 5 }}>{label}</div>
        <div style={{ fontSize: 15, fontWeight: 600, color, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
          {value}
        </div>
      </div>
      <Btn
        variant="ghost"
        size="sm"
        icon="refresh"
        disabled={disabled}
        onClick={onAction}
      >
        {actionLabel}
      </Btn>
    </div>
  );
}
