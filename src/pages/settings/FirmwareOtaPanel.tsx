import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { APP_VERSION } from '../../lib/appVersion';
import {
  exportDiagnosticPackage,
  getFirmwareOtaPreflightSnapshot,
  transferFirmwareOtaBle,
} from '../../lib/ipc';
import {
  evaluateFirmwareOtaPreflight,
  firmwareOtaFailureNextStep,
  firmwareOtaReducer,
  initialFirmwareOtaState,
  validateFirmwareOtaPackage,
  type FirmwareOtaBlocker,
  type FirmwareOtaDeviceSnapshot,
  type FirmwareOtaManifest,
  type FirmwareOtaPreflightSnapshot,
  type FirmwareOtaUserState,
} from '../../lib/firmwareOta';
import { Btn, Pill, type PillTone } from '../_atoms';
import type { EmbeddedBleProbeStatus } from './EmbeddedBleStatusPanel';

const EXPECTED_HARDWARE_REVISION = 'keyboard-v1';

interface SelectedPackage {
  manifest: FirmwareOtaManifest;
  manifestText: string;
  firmwareBytes: Uint8Array;
  firmwareSha256: string;
  warnings: string[];
}

export function FirmwareOtaPanel({
  supported,
  bleStatus,
  onProbe,
}: {
  supported: boolean;
  bleStatus: EmbeddedBleProbeStatus;
  onProbe: () => void;
}) {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [state, dispatch] = useReducer(firmwareOtaReducer, initialFirmwareOtaState);
  const [selectedPackage, setSelectedPackage] = useState<SelectedPackage | null>(null);
  const [blockers, setBlockers] = useState<FirmwareOtaBlocker[]>([]);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [otaSnapshot, setOtaSnapshot] = useState<FirmwareOtaPreflightSnapshot | null>(null);
  const [snapshotError, setSnapshotError] = useState<string | null>(null);
  const [diagnosticStatus, setDiagnosticStatus] = useState<'idle' | 'busy' | 'ok' | 'err'>('idle');

  const transferActive = state.userState === 'transferring' || state.userState === 'rebooting' || state.userState === 'verifying';
  const statusTone = userStateTone(state.userState);
  const statusLabel = userStateLabel(state.userState, t);
  const refreshOtaSnapshot = useCallback(async () => {
    if (!supported) {
      const unsupported = makeDisconnectedSnapshot('Firmware OTA is only supported on Windows Listener BLE.');
      setOtaSnapshot(unsupported);
      setSnapshotError(null);
      return unsupported;
    }
    try {
      const snapshot = await getFirmwareOtaPreflightSnapshot();
      setOtaSnapshot(snapshot);
      setSnapshotError(null);
      return snapshot;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const failed = makeDisconnectedSnapshot(message);
      setOtaSnapshot(failed);
      setSnapshotError(message);
      return failed;
    }
  }, [supported]);

  useEffect(() => {
    if (!selectedPackage || bleStatus === 'checking') return;
    void refreshOtaSnapshot();
  }, [bleStatus, refreshOtaSnapshot, selectedPackage]);

  const preflight = useMemo(() => {
    if (!selectedPackage) return null;
    const snapshot = otaSnapshot ?? makeDisconnectedSnapshot('Refresh Listener BLE status before starting OTA.');
    return evaluateFirmwareOtaPreflight({
      manifest: selectedPackage.manifest,
      desktopVersion: APP_VERSION,
      recordingActive: snapshot.recordingActive,
      transferActive,
      device: snapshot.device,
    });
  }, [otaSnapshot, selectedPackage, transferActive]);
  const effectiveBlockers = blockers.length > 0 ? blockers : preflight?.blockers ?? [];
  const canStart = !!selectedPackage && state.userState === 'ready' && effectiveBlockers.length === 0 && !transferActive;

  const onFilesSelected = async (files: FileList | null) => {
    setValidationErrors([]);
    setBlockers([]);
    setSelectedPackage(null);
    if (!files || files.length === 0) return;
    dispatch({ type: 'check' });
    const selected = Array.from(files);
    const manifestFile = selected.find(file => file.name === 'ota_manifest.json' || file.name.endsWith('.json'));
    const firmwareFile = selected.find(file => file.name === 'firmware_ota.bin' || file.name.endsWith('.bin'));
    if (!manifestFile || !firmwareFile) {
      dispatch({ type: 'failed', failureCode: 'manifestMismatch', message: 'Missing ota_manifest.json or firmware_ota.bin.' });
      setValidationErrors([t('settings.recording.firmwareOtaMissingFiles', '请选择同一目录里的 ota_manifest.json 和 firmware_ota.bin。')]);
      return;
    }

    try {
      const manifestText = await manifestFile.text();
      const firmwareBytes = new Uint8Array(await firmwareFile.arrayBuffer());
      const result = await validateFirmwareOtaPackage(manifestText, firmwareBytes, {
        desktopVersion: APP_VERSION,
        expectedHardwareRevision: EXPECTED_HARDWARE_REVISION,
      });
      if (!result.ok || !result.manifest || !result.firmwareSha256) {
        dispatch({ type: 'failed', failureCode: result.errors.some(error => error.includes('SHA256')) ? 'hashFailure' : 'manifestMismatch', message: result.errors[0] ?? 'Invalid OTA package.' });
        setValidationErrors(result.errors);
        return;
      }
      setSelectedPackage({
        manifest: result.manifest,
        manifestText,
        firmwareBytes,
        firmwareSha256: result.firmwareSha256,
        warnings: result.warnings,
      });
      void refreshOtaSnapshot();
      dispatch({ type: 'ready' });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      dispatch({ type: 'failed', failureCode: 'manifestMismatch', message });
      setValidationErrors([message]);
    } finally {
      if (inputRef.current) inputRef.current.value = '';
    }
  };

  const startUpdate = async () => {
    if (!selectedPackage) return;
    const snapshot = await refreshOtaSnapshot();
    const check = evaluateFirmwareOtaPreflight({
      manifest: selectedPackage.manifest,
      desktopVersion: APP_VERSION,
      recordingActive: snapshot.recordingActive,
      transferActive,
      device: snapshot.device,
    });
    if (!check.ok) {
      setBlockers(check.blockers);
      dispatch({ type: 'failed', failureCode: 'deviceRejected', message: check.blockers[0]?.message ?? 'Preflight blocked.' });
      return;
    }

    setBlockers([]);
    try {
      dispatch({ type: 'startTransfer' });
      let progress = 1;
      const progressTimer = window.setInterval(() => {
        progress = Math.min(95, progress + 18);
        dispatch({ type: 'transferProgress', progress });
      }, 260);
      let transferResult: Awaited<ReturnType<typeof transferFirmwareOtaBle>> | null = null;
      try {
        transferResult = await transferFirmwareOtaBle({
          manifest: selectedPackage.manifest,
          firmwareBytes: selectedPackage.firmwareBytes,
          expectedSha256: selectedPackage.firmwareSha256,
        });
      } finally {
        window.clearInterval(progressTimer);
      }
      dispatch({ type: 'transferComplete' });
      await delay(450);
      dispatch({ type: 'deviceReconnected' });
      await delay(450);
      if (transferResult?.confirmedVersion === selectedPackage.manifest.version) {
        dispatch({ type: 'verified' });
      } else {
        dispatch({
          type: 'failed',
          failureCode: 'versionNotConfirmed',
          message: 'Device rebooted but the new firmware version was not confirmed.',
        });
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      dispatch({
        type: 'failed',
        failureCode: message.toLowerCase().includes('disconnect') ? 'bleDisconnected' : 'deviceRejected',
        message,
      });
    }
  };

  const retryAfterFailure = () => {
    setBlockers([]);
    setValidationErrors([]);
    if (selectedPackage) {
      dispatch({ type: 'ready' });
    } else {
      dispatch({ type: 'retry' });
    }
  };

  const exportDiagnostics = async () => {
    setDiagnosticStatus('busy');
    try {
      await exportDiagnosticPackage('listener-type-ota-diagnostics.json');
      setDiagnosticStatus('ok');
    } catch (error) {
      console.warn('[firmware-ota] diagnostic export failed', error);
      setDiagnosticStatus('err');
    }
  };

  return (
    <div
      style={{
        marginTop: 8,
        padding: '14px 16px',
        borderRadius: 8,
        border: '0.5px solid var(--ol-line)',
        background: 'var(--ol-surface-2)',
        display: 'flex',
        flexDirection: 'column',
        gap: 12,
      }}
    >
      <input
        ref={inputRef}
        type="file"
        multiple
        accept=".json,.bin,application/json,application/octet-stream"
        onChange={event => void onFilesSelected(event.target.files)}
        style={{ display: 'none' }}
      />
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('settings.recording.firmwareOtaTitle', '设备固件')}
          </div>
          <Pill tone={statusTone} size="sm">{statusLabel}</Pill>
        </div>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
          <Btn variant="ghost" size="sm" icon="refresh" onClick={onProbe} disabled={!supported || transferActive}>
            {t('common.refresh')}
          </Btn>
          <Btn variant="ghost" size="sm" icon="refresh" onClick={() => void refreshOtaSnapshot()} disabled={!supported || transferActive}>
            {t('settings.recording.firmwareOtaRefreshReadiness', '刷新升级条件')}
          </Btn>
          <Btn variant="ghost" size="sm" icon="doc" onClick={() => inputRef.current?.click()} disabled={transferActive}>
            {t('settings.recording.firmwareOtaChoosePackage', '选择包')}
          </Btn>
          <Btn variant="blue" size="sm" icon="download" onClick={() => void startUpdate()} disabled={!canStart}>
            {t('settings.recording.firmwareOtaStart', '更新')}
          </Btn>
        </div>
      </div>

      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
        {t(
          'settings.recording.firmwareOtaDesc',
          '选择 firmware repo 生成的 OTA 包；更新时会暂停录音入口，只走独立 BLE OTA 通道，不占用 BLE audio 或 HID。',
        )}
      </div>

      {selectedPackage && (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, minmax(0, 1fr))', gap: 8 }}>
          <FirmwareOtaFact label={t('settings.recording.firmwareOtaVersion', '版本')} value={selectedPackage.manifest.version} />
          <FirmwareOtaFact label={t('settings.recording.firmwareOtaChannel', '渠道')} value={selectedPackage.manifest.channel} />
          <FirmwareOtaFact label={t('settings.recording.firmwareOtaSize', '大小')} value={`${selectedPackage.manifest.fileSizeBytes} bytes`} />
        </div>
      )}

      {selectedPackage && (
        <FirmwareOtaReadinessSummary snapshot={otaSnapshot} snapshotError={snapshotError} t={t} />
      )}

      {state.userState === 'transferring' && (
        <div style={{ height: 6, borderRadius: 999, overflow: 'hidden', background: 'var(--ol-control-track)' }}>
          <div
            style={{
              width: `${Math.max(2, state.progress)}%`,
              height: '100%',
              background: 'var(--ol-blue)',
              transition: 'width 0.18s var(--ol-motion-quick)',
            }}
          />
        </div>
      )}

      {(validationErrors.length > 0 || effectiveBlockers.length > 0 || state.failureCode) && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {validationErrors.map(error => (
            <div key={error} style={{ fontSize: 11.5, color: 'var(--ol-err)', lineHeight: 1.5 }}>
              {error}
            </div>
          ))}
          {effectiveBlockers.map(item => (
            <div key={item.code} style={{ fontSize: 11.5, color: 'var(--ol-err)', lineHeight: 1.5 }}>
              {item.message} {item.nextStep}
            </div>
          ))}
          {state.failureCode && (
            <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
              {firmwareOtaFailureNextStep(state.failureCode)}
            </div>
          )}
        </div>
      )}

      {selectedPackage?.warnings.map(warning => (
        <div key={warning} style={{ fontSize: 11.5, color: '#b45309', lineHeight: 1.5 }}>
          {warning}
        </div>
      ))}

      {(state.userState === 'failed' || state.userState === 'rolledBack') && (
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
          <Btn variant="soft" size="sm" icon="refresh" onClick={retryAfterFailure}>
            {t('common.retry')}
          </Btn>
          <Btn variant="ghost" size="sm" icon="doc" onClick={() => void exportDiagnostics()} disabled={diagnosticStatus === 'busy'}>
            {diagnosticStatus === 'busy'
              ? t('modal.about.exporting')
              : t('modal.about.exportDiagnosticPackageBtn')}
          </Btn>
          {diagnosticStatus === 'ok' && <span style={{ fontSize: 11, color: 'var(--ol-ok)' }}>{t('modal.about.exportSuccess')}</span>}
          {diagnosticStatus === 'err' && <span style={{ fontSize: 11, color: 'var(--ol-err)' }}>{t('modal.about.exportFailed')}</span>}
        </div>
      )}
    </div>
  );
}

function FirmwareOtaFact({ label, value }: { label: string; value: string }) {
  return (
    <div
      style={{
        minWidth: 0,
        padding: '8px 10px',
        borderRadius: 7,
        border: '0.5px solid var(--ol-line-soft)',
        background: 'var(--ol-control-track)',
      }}
    >
      <div style={{ fontSize: 10.5, color: 'var(--ol-ink-4)', marginBottom: 3 }}>{label}</div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink)', fontFamily: 'var(--ol-font-mono)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        {value}
      </div>
    </div>
  );
}

function FirmwareOtaReadinessSummary({
  snapshot,
  snapshotError,
  t,
}: {
  snapshot: FirmwareOtaPreflightSnapshot | null;
  snapshotError: string | null;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  const device = snapshot?.device;
  const rows: Array<[string, string]> = [
    [t('settings.recording.firmwareOtaDeviceConnected', '连接'), device?.connected ? 'connected' : 'not ready'],
    [t('settings.recording.firmwareOtaDeviceHardware', '硬件'), device?.hardwareRevision ?? 'unknown'],
    [t('settings.recording.firmwareOtaDeviceFirmware', '固件'), device?.firmwareVersion ?? 'unknown'],
    [t('settings.recording.firmwareOtaDevicePower', '供电'), formatPower(device)],
    [t('settings.recording.firmwareOtaDictationPhase', '录音'), snapshot?.recordingActive ? snapshot.dictationPhase : 'idle'],
  ];

  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(108px, 1fr))', gap: 8 }}>
      {rows.map(([label, value]) => <FirmwareOtaFact key={label} label={label} value={value} />)}
      {snapshotError && (
        <div style={{ gridColumn: '1 / -1', fontSize: 11.5, color: 'var(--ol-err)', lineHeight: 1.5 }}>
          {snapshotError}
        </div>
      )}
    </div>
  );
}

function formatPower(device: FirmwareOtaDeviceSnapshot | null | undefined): string {
  if (!device) return 'unknown';
  if (device.usbPowered === true) return 'USB';
  if (device.usbPowered === false && typeof device.batteryPercent === 'number') {
    return `${device.batteryPercent}%`;
  }
  return 'unknown';
}

function makeDisconnectedSnapshot(detail: string): FirmwareOtaPreflightSnapshot {
  return {
    recordingActive: false,
    dictationPhase: 'unknown',
    device: {
      connected: false,
      hardwareRevision: null,
      firmwareVersion: null,
      capabilities: [],
      batteryPercent: null,
      usbPowered: null,
      detail,
    },
  };
}

function userStateTone(state: FirmwareOtaUserState): PillTone {
  switch (state) {
    case 'ready':
    case 'success':
      return 'ok';
    case 'checking':
    case 'transferring':
    case 'rebooting':
    case 'verifying':
      return 'blue';
    case 'failed':
    case 'rolledBack':
      return 'err';
    case 'idle':
      return 'outline';
  }
}

function userStateLabel(state: FirmwareOtaUserState, t: ReturnType<typeof useTranslation>['t']): string {
  switch (state) {
    case 'idle':
      return t('settings.recording.firmwareOtaIdle', '未选择');
    case 'checking':
      return t('settings.recording.firmwareOtaChecking', '检查中');
    case 'ready':
      return t('settings.recording.firmwareOtaReady', '可更新');
    case 'transferring':
      return t('settings.recording.firmwareOtaTransferring', '传输中');
    case 'rebooting':
      return t('settings.recording.firmwareOtaRebooting', '重启中');
    case 'verifying':
      return t('settings.recording.firmwareOtaVerifying', '确认中');
    case 'success':
      return t('settings.recording.firmwareOtaSuccess', '成功');
    case 'failed':
      return t('settings.recording.firmwareOtaFailed', '失败');
    case 'rolledBack':
      return t('settings.recording.firmwareOtaRolledBack', '已回滚');
  }
}

function delay(ms: number): Promise<void> {
  return new Promise(resolve => window.setTimeout(resolve, ms));
}
