import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { APP_VERSION } from '../../lib/appVersion';
import {
  exportDiagnosticPackage,
  getFirmwareOtaPreflightSnapshot,
  loadFirmwareOtaPackage,
  transferFirmwareOtaBle,
} from '../../lib/ipc';
import {
  evaluateFirmwareOtaPreflight,
  firmwareOtaConfirmedVersionMatches,
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
import type { EmbeddedBleProbeStatus } from '../../components/EmbeddedBleStatusPanel';

const EXPECTED_HARDWARE_REVISION = 'keyboard-v1';

interface SelectedPackage {
  manifest: FirmwareOtaManifest;
  manifestText: string;
  firmwareBytes: Uint8Array;
  firmwareSha256: string;
  warnings: string[];
  sourceLabel: string;
}

export function FirmwareOtaPanel({
  supported,
  bleStatus,
}: {
  supported: boolean;
  bleStatus: EmbeddedBleProbeStatus;
}) {
  const { t, i18n } = useTranslation();
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [state, dispatch] = useReducer(firmwareOtaReducer, initialFirmwareOtaState);
  const [selectedPackage, setSelectedPackage] = useState<SelectedPackage | null>(null);
  const [blockers, setBlockers] = useState<FirmwareOtaBlocker[]>([]);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [otaSnapshot, setOtaSnapshot] = useState<FirmwareOtaPreflightSnapshot | null>(null);
  const [snapshotError, setSnapshotError] = useState<string | null>(null);
  const [snapshotRefreshing, setSnapshotRefreshing] = useState(false);
  const [diagnosticStatus, setDiagnosticStatus] = useState<'idle' | 'busy' | 'ok' | 'err'>('idle');
  const [progressBytes, setProgressBytes] = useState<{ sent: number; total: number } | null>(null);

  const transferActive = state.userState === 'transferring' || state.userState === 'rebooting' || state.userState === 'verifying';
  const statusTone = userStateTone(state.userState);
  const statusLabel = userStateLabel(state.userState, t);
  const refreshOtaSnapshot = useCallback(async () => {
    setSnapshotRefreshing(true);
    if (!supported) {
      const unsupported = makeDisconnectedSnapshot('Firmware OTA is only supported on Windows Listener BLE.');
      setOtaSnapshot(unsupported);
      setSnapshotError(null);
      setSnapshotRefreshing(false);
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
    } finally {
      setSnapshotRefreshing(false);
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
  const effectiveBlockers = blockers.length > 0 ? blockers : transferActive ? [] : preflight?.blockers ?? [];
  const canStart = !!selectedPackage && state.userState === 'ready' && effectiveBlockers.length === 0 && !transferActive;

  const onFilesSelected = async (files: FileList | null) => {
    setValidationErrors([]);
    setBlockers([]);
    setSelectedPackage(null);
    setProgressBytes(null);
    if (!files || files.length === 0) return;
    dispatch({ type: 'check' });
    const selected = Array.from(files);
    const manifestFile = selected.find(file => file.name === 'ota_manifest.json' || (file.name.endsWith('.json') && file.webkitRelativePath.includes('ota_manifest')));
    const firmwareFile = selected.find(file => file.name === 'firmware_ota.bin' || (file.name.endsWith('.bin') && !file.name.startsWith('.')));
    if (!manifestFile || !firmwareFile) {
      dispatch({ type: 'failed', failureCode: 'manifestMismatch', message: 'Missing ota_manifest.json or firmware_ota.bin.' });
      setValidationErrors([t('settings.recording.firmwareOtaMissingFiles', 'OTA 包目录里需要包含 ota_manifest.json 和 firmware_ota.bin。')]);
      return;
    }

    try {
      const manifestText = await manifestFile.text();
      const firmwareBytes = new Uint8Array(await firmwareFile.arrayBuffer());
      await acceptPackage(manifestText, firmwareBytes, 'OTA package directory');
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      dispatch({ type: 'failed', failureCode: 'manifestMismatch', message });
      setValidationErrors([message]);
    } finally {
      if (inputRef.current) inputRef.current.value = '';
    }
  };

  const chooseZipPackage = async () => {
    setValidationErrors([]);
    setBlockers([]);
    setSelectedPackage(null);
    setProgressBytes(null);
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: 'Listener OTA package', extensions: ['zip'] }],
      });
      if (typeof selected !== 'string') return;
      dispatch({ type: 'check' });
      const payload = await loadFirmwareOtaPackage(selected);
      await acceptPackage(payload.manifestText, new Uint8Array(payload.firmwareBytes), payload.sourceLabel);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      dispatch({ type: 'failed', failureCode: 'manifestMismatch', message });
      setValidationErrors([message]);
    }
  };

  const acceptPackage = async (manifestText: string, firmwareBytes: Uint8Array, sourceLabel: string) => {
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
      sourceLabel,
    });
    void refreshOtaSnapshot();
    dispatch({ type: 'ready' });
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
      setProgressBytes({ sent: 0, total: selectedPackage.firmwareBytes.byteLength });
      let transferResult: Awaited<ReturnType<typeof transferFirmwareOtaBle>> | null = null;
      const unlisten = await listen<{ bytesSent: number; bytesTotal: number }>('firmware-ota:progress', event => {
        const bytesTotal = Math.max(1, event.payload.bytesTotal);
        const bytesSent = Math.min(event.payload.bytesSent, bytesTotal);
        setProgressBytes({ sent: bytesSent, total: bytesTotal });
        const pct = Math.round((bytesSent / bytesTotal) * 100);
        dispatch({ type: 'transferProgress', progress: Math.min(pct, 99) });
      });
      try {
        transferResult = await transferFirmwareOtaBle({
          manifest: selectedPackage.manifest,
          firmwareBytes: selectedPackage.firmwareBytes,
          expectedSha256: selectedPackage.firmwareSha256,
        });
      } finally {
        unlisten();
      }
      dispatch({ type: 'transferComplete' });
      await delay(450);
      dispatch({ type: 'deviceReconnected' });
      await delay(450);
      if (firmwareOtaConfirmedVersionMatches(transferResult?.confirmedVersion, selectedPackage.manifest.version)) {
        const confirmedVersion = transferResult?.confirmedVersion?.trim() ?? null;
        if (confirmedVersion) {
          setOtaSnapshot(previous => snapshotWithFirmwareVersion(previous, confirmedVersion));
        }
        dispatch({ type: 'verified' });
        void refreshOtaSnapshot();
      } else {
        const confirmedVersion = transferResult?.confirmedVersion?.trim();
        dispatch({
          type: 'failed',
          failureCode: 'versionNotConfirmed',
          message: confirmedVersion
            ? `Device reported firmware ${confirmedVersion}, not ${selectedPackage.manifest.version}.`
            : 'Device firmware version was not confirmed after the OTA reboot window.',
        });
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      dispatch({
        type: 'failed',
        failureCode: message.toLowerCase().includes('disconnect') ? 'bleDisconnected' : 'deviceRejected',
        message,
      });
    } finally {
      setProgressBytes(null);
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
        {...{ webkitdirectory: '', directory: '' } as React.InputHTMLAttributes<HTMLInputElement>}
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
          <Btn variant="ghost" size="sm" icon="doc" onClick={() => void chooseZipPackage()} disabled={transferActive}>
            {t('settings.recording.firmwareOtaChoosePackage', '选择 OTA zip')}
          </Btn>
          <Btn variant="soft" size="sm" icon="archive" onClick={() => inputRef.current?.click()} disabled={transferActive}>
            {t('settings.recording.firmwareOtaChoosePackageDir', '目录')}
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
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', gap: 8 }}>
          <FirmwareOtaFact label={t('settings.recording.firmwareOtaPackageVersion', '升级包版本')} value={selectedPackage.manifest.version} />
          <FirmwareOtaFact label={t('settings.recording.firmwareOtaChannel', '渠道')} value={selectedPackage.manifest.channel} />
          <FirmwareOtaFact label={t('settings.recording.firmwareOtaSize', '升级包大小')} value={formatKb(selectedPackage.manifest.fileSizeBytes)} />
        </div>
      )}

      {selectedPackage && (
        <FirmwareOtaReadinessSummary
          snapshot={otaSnapshot}
          snapshotError={snapshotError}
          refreshing={snapshotRefreshing}
          onRefresh={() => void refreshOtaSnapshot()}
          t={t}
        />
      )}

      {(state.userState === 'transferring' || state.userState === 'rebooting' || state.userState === 'verifying') && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <div style={{ flex: 1, height: 6, borderRadius: 999, overflow: 'hidden', background: 'var(--ol-control-track)' }}>
            <div
              style={{
                width: `${Math.max(2, state.progress)}%`,
                height: '100%',
                background: 'var(--ol-blue)',
                transition: 'width 0.3s ease',
              }}
            />
          </div>
          <span style={{ fontSize: 11, color: 'var(--ol-ink-4)', minWidth: 92, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
            {state.userState === 'transferring' && progressBytes
              ? `${formatKb(progressBytes.sent)} / ${formatKb(progressBytes.total)}`
              : state.userState === 'transferring' ? `${state.progress}%` : ''}
          </span>
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
              {formatFirmwareOtaBlocker(item, i18n.resolvedLanguage ?? i18n.language)}
            </div>
          ))}
          {state.failureCode && (
            <>
              {state.message && (
                <div style={{ fontSize: 11.5, color: 'var(--ol-err)', lineHeight: 1.5 }}>
                  {state.message}
                </div>
              )}
              <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
                {formatFirmwareOtaFailureNextStep(state.failureCode, i18n.resolvedLanguage ?? i18n.language)}
              </div>
            </>
          )}
        </div>
      )}

      {selectedPackage?.warnings.map(warning => (
        <div key={warning} style={{ fontSize: 11.5, color: '#b45309', lineHeight: 1.5 }}>
          {formatFirmwareOtaWarning(warning, i18n.resolvedLanguage ?? i18n.language)}
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
  refreshing,
  onRefresh,
  t,
}: {
  snapshot: FirmwareOtaPreflightSnapshot | null;
  snapshotError: string | null;
  refreshing: boolean;
  onRefresh: () => void;
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
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
        <div style={{ fontSize: 11, fontWeight: 600, color: 'var(--ol-ink-3)' }}>
          {t('settings.recording.firmwareOtaDeviceStatus', '当前设备')}
        </div>
        <Btn variant="soft" size="sm" icon="refresh" onClick={onRefresh} disabled={refreshing}>
          {refreshing
            ? t('settings.recording.firmwareOtaRefreshing', '查询中')
            : t('settings.recording.firmwareOtaRefreshDevice', '查询固件版本')}
        </Btn>
      </div>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(108px, 1fr))', gap: 8 }}>
        {rows.map(([label, value]) => <FirmwareOtaFact key={label} label={label} value={value} />)}
      </div>
      {snapshotError && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-err)', lineHeight: 1.5 }}>
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

function snapshotWithFirmwareVersion(
  snapshot: FirmwareOtaPreflightSnapshot | null,
  firmwareVersion: string,
): FirmwareOtaPreflightSnapshot {
  const base = snapshot ?? makeDisconnectedSnapshot('Firmware version confirmed after OTA.');
  return {
    ...base,
    device: {
      ...base.device,
      connected: true,
      firmwareVersion,
      detail: 'Firmware version confirmed from device after OTA.',
    },
  };
}

function formatKb(bytes: number): string {
  return `${Math.max(0, bytes / 1024).toFixed(1)} KB`;
}

function formatFirmwareOtaBlocker(
  blocker: FirmwareOtaBlocker,
  language: string,
): string {
  switch (blocker.code) {
    case 'deviceDisconnected':
      return localizeOtaText(language, '设备还没有准备好 OTA。请确认 Listener 已在 Windows 蓝牙中连接，然后重新查询设备状态。', 'Device is not ready for OTA. Connect Listener in Windows Bluetooth, then query device status again.');
    case 'recordingActive':
      return localizeOtaText(language, '当前还在录音，请先停止或取消录音再更新固件。', 'Recording is still active. Stop or cancel recording before updating firmware.');
    case 'transferActive':
      return localizeOtaText(language, '已有一次升级正在进行，请等待完成后再试。', 'Another update is already running. Wait for it to finish and retry.');
    case 'deviceStatusUnknown':
      return localizeOtaText(language, '设备硬件版本未知，请重新查询设备状态。', 'Device hardware revision is unknown. Query device status again.');
    case 'batteryLow':
      return localizeOtaText(language, '设备电量过低，请接入 USB 或充到 20% 以上再更新。', 'Battery is too low. Connect USB power or charge above 20%.');
    case 'powerUnknown':
      return localizeOtaText(language, '设备供电状态未知，请接入 USB 后再更新。', 'Power state is unknown. Connect USB power before updating.');
    case 'hardwareMismatch':
      return localizeOtaText(language, '升级包硬件版本与当前设备不匹配，请选择对应硬件的 OTA 包。', 'This OTA package targets a different hardware revision. Choose the package for this device.');
    case 'missingCapability':
      return localizeOtaText(language, '当前固件没有开放 OTA 能力，请先用 USB factory 包刷一次后再试 OTA。', 'Current firmware does not advertise OTA support. Flash the USB factory package once, then retry OTA.');
    case 'minDesktopVersion':
      return localizeOtaText(language, '当前 Listener Type 版本太旧，请先升级桌面端。', 'Listener Type is too old for this package. Update the desktop app first.');
    case 'sameVersion':
      return localizeOtaText(language, '升级包版本不高于当前设备固件。', 'The OTA package is not newer than the device firmware.');
  }
}

function formatFirmwareOtaFailureNextStep(
  code: NonNullable<ReturnType<typeof firmwareOtaReducer>['failureCode']>,
  language: string,
): string {
  switch (code) {
    case 'bleDisconnected':
      return localizeOtaText(language, '请重新连接 Listener，查询设备状态后再试。', 'Reconnect Listener, query device status, then retry.');
    case 'manifestMismatch':
      return localizeOtaText(language, '请重新选择同一个 OTA 包中的 manifest 和固件文件。', 'Choose the manifest and firmware from the same OTA package.');
    case 'hashFailure':
      return localizeOtaText(language, '升级包校验失败，请重新生成或下载 OTA 包。', 'OTA package verification failed. Rebuild or download the package again.');
    case 'deviceRejected':
      return localizeOtaText(language, '设备拒绝升级，请导出诊断包并检查设备状态。', 'The device rejected the update. Export diagnostics and check device status.');
    case 'versionNotConfirmed':
      return localizeOtaText(language, '等待设备重新连接后查询固件版本；如果仍未变化，请重试或导出诊断包。', 'Wait for the device to reconnect, then query firmware version. Retry or export diagnostics if it did not change.');
    case 'rolledBack':
      return localizeOtaText(language, '设备已经回滚到旧固件，请先导出诊断包再重试。', 'The device rolled back to the previous firmware. Export diagnostics before retrying.');
  }
}

function formatFirmwareOtaWarning(
  warning: string,
  language: string,
): string {
  if (warning.includes('not newer than the connected firmware version')) {
    return localizeOtaText(language, '升级包版本不高于当前设备固件；测试阶段允许重刷同版本。', 'The OTA package is not newer than the device firmware. Same-version reflashing is allowed during testing.');
  }
  return warning;
}

function localizeOtaText(language: string, zh: string, en: string): string {
  return language.toLowerCase().startsWith('zh') ? zh : en;
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
