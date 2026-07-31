import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useReducer, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { APP_VERSION } from '../../lib/appVersion';
import {
  exportDiagnosticPackage,
  flashWiredFirmwarePackage,
  getFirmwareOtaPreflightSnapshot,
  listWiredFirmwarePorts,
  loadFirmwareOtaPackage,
  loadWiredFirmwarePackage,
  transferFirmwareOtaBle,
  type WiredFirmwareFlashResult,
  type WiredFirmwarePackagePayload,
  type WiredFirmwareProgressPayload,
  type WiredFirmwareSerialPort,
} from '../../lib/ipc';
import {
  compareVersionish,
  estimateFirmwareOtaTransferSpeedKibPerSec,
  evaluateFirmwareOtaPreflight,
  firmwareOtaConfirmedVersionMatches,
  firmwareOtaConfirmedVersionLooksRolledBack,
  firmwareOtaReducer,
  firmwareOtaRollbackVersionFromText,
  firmwareOtaSnapshotSatisfiesVersionRefreshFallback,
  firmwareOtaVersionNotConfirmedAction,
  formatFirmwareOtaTransferSpeed,
  initialFirmwareOtaState,
  LISTENER_OTA_V1_TRANSPORT_BOUNDARY,
  validateFirmwareOtaPackage,
  type FirmwareOtaBlocker,
  type FirmwareOtaDeviceSnapshot,
  type FirmwareOtaManifest,
  type FirmwareOtaPreflightSnapshot,
  type FirmwareOtaUserState,
} from '../../lib/firmwareOta';
import { Icon } from '../../components/Icon';
import { Btn, Pill, type PillTone } from '../_atoms';

const EXPECTED_HARDWARE_REVISION = 'keyboard-v2-n16r8';
const OTA_VERSION_QUERY_TIMEOUT_MS = 15_000;
const OTA_VERSION_QUERY_POLL_MS = 700;
const OTA_PREFLIGHT_SNAPSHOT_FRESH_MS = 60_000;

interface SelectedPackage {
  path: string;
  manifest: FirmwareOtaManifest;
  manifestText: string;
  firmwareBytes: Uint8Array;
  firmwareSha256: string;
  warnings: string[];
  sourceLabel: string;
  sourceKind: 'zip' | 'directory';
}

export function FirmwareOtaPanel({
  supported,
}: {
  supported: boolean;
}) {
  const { t, i18n } = useTranslation();
  const [state, dispatch] = useReducer(firmwareOtaReducer, initialFirmwareOtaState);
  const [firmwareMode, setFirmwareMode] = useState<'ble' | 'wired'>('ble');
  const [wiredBusy, setWiredBusy] = useState(false);
  const wiredRef = useRef<FirmwareWiredFlashHandle>(null);
  const [wiredAction, setWiredAction] = useState<{ canFlash: boolean; isFlashing: boolean }>({ canFlash: false, isFlashing: false });
  const [selectedPackage, setSelectedPackage] = useState<SelectedPackage | null>(null);
  const [blockers, setBlockers] = useState<FirmwareOtaBlocker[]>([]);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [otaSnapshot, setOtaSnapshot] = useState<FirmwareOtaPreflightSnapshot | null>(null);
  const [otaSnapshotFetchedAtMs, setOtaSnapshotFetchedAtMs] = useState<number | null>(null);
  const [snapshotError, setSnapshotError] = useState<string | null>(null);
  const [snapshotRefreshing, setSnapshotRefreshing] = useState(false);
  const [diagnosticStatus, setDiagnosticStatus] = useState<'idle' | 'busy' | 'ok' | 'err'>('idle');
  const [progressBytes, setProgressBytes] = useState<{ sent: number; total: number } | null>(null);
  /** Rough live transfer rate shown above the progress bar during BLE OTA. */
  const [transferSpeedKibPerSec, setTransferSpeedKibPerSec] = useState<number | null>(null);
  const transferSpeedSamplesRef = useRef<Array<{ tMs: number; bytes: number }>>([]);
  const otaStartInFlightRef = useRef(false);

  const transferActive = state.userState === 'transferring' || state.userState === 'rebooting' || state.userState === 'verifying';
  const firmwareActionBusy = transferActive || wiredBusy;
  const statusTone = userStateTone(state.userState);
  const statusLabel = userStateLabel(state.userState, t);

  const setSnapshotResult = useCallback((snapshot: FirmwareOtaPreflightSnapshot, error: string | null) => {
    setOtaSnapshot(snapshot);
    setOtaSnapshotFetchedAtMs(Date.now());
    setSnapshotError(error);
  }, []);

  const refreshOtaSnapshot = useCallback(async (options: { waitForFirmwareVersion?: boolean; protocolName?: string | null } = {}) => {
    const waitForFirmwareVersion = options.waitForFirmwareVersion ?? false;
    const protocolName = options.protocolName ?? selectedPackage?.manifest.protocolName ?? null;
    setSnapshotRefreshing(true);
    if (transferActive) {
      const active = makeActiveFirmwareOtaSnapshot();
      setSnapshotResult(active, null);
      setSnapshotRefreshing(false);
      return active;
    }
    if (!supported) {
      const unsupported = makeDisconnectedSnapshot('Firmware OTA is only supported on Windows Listener BLE.');
      setSnapshotResult(unsupported, null);
      setSnapshotRefreshing(false);
      return unsupported;
    }
    const timeoutMessage = t('settings.recording.firmwareOtaRefreshTimeout', '查询固件版本超时，请重试。');
    const deadline = Date.now() + OTA_VERSION_QUERY_TIMEOUT_MS;
    let lastSnapshot: FirmwareOtaPreflightSnapshot | null = null;
    let lastError: string | null = null;
    try {
      do {
        try {
          const rpcTimeoutMs = Math.max(500, Math.min(OTA_VERSION_QUERY_TIMEOUT_MS, deadline - Date.now()));
          const snapshot = await withTimeout(getFirmwareOtaPreflightSnapshot({ protocolName }), rpcTimeoutMs, timeoutMessage);
          lastSnapshot = snapshot;
          setSnapshotResult(snapshot, null);
          if (
            !waitForFirmwareVersion ||
            snapshot.device.firmwareVersion ||
            firmwareOtaSnapshotSatisfiesVersionRefreshFallback(snapshot, protocolName)
          ) {
            return snapshot;
          }
        } catch (error) {
          lastError = error instanceof Error ? error.message : String(error);
          if (!waitForFirmwareVersion) {
            const failed = makeDisconnectedSnapshot(lastError);
            setSnapshotResult(failed, lastError);
            return failed;
          }
        }
        await delay(OTA_VERSION_QUERY_POLL_MS);
      } while (Date.now() <= deadline);

      if (lastSnapshot) {
        setSnapshotResult(lastSnapshot, timeoutMessage);
        return lastSnapshot;
      }
      const failed = makeDisconnectedSnapshot(lastError ?? timeoutMessage);
      setSnapshotResult(failed, lastError ?? timeoutMessage);
      return failed;
    } finally {
      setSnapshotRefreshing(false);
    }
  }, [selectedPackage?.manifest.protocolName, setSnapshotResult, supported, t, transferActive]);

  const getFreshOtaSnapshot = useCallback(() => {
    if (!otaSnapshot || snapshotError || otaSnapshotFetchedAtMs === null) return null;
    if (Date.now() - otaSnapshotFetchedAtMs > OTA_PREFLIGHT_SNAPSHOT_FRESH_MS) return null;
    return otaSnapshot;
  }, [otaSnapshot, otaSnapshotFetchedAtMs, snapshotError]);

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
  const preflightPending = !!selectedPackage && (snapshotRefreshing || !otaSnapshot);
  const effectiveBlockers = blockers.length > 0
    ? blockers
    : transferActive || preflightPending
      ? []
      : preflight?.blockers ?? [];
  const visiblePreflightBlockers = snapshotError
    ? effectiveBlockers.filter(item => item.code !== 'deviceDisconnected')
    : effectiveBlockers;
  const dynamicWarnings = useMemo(() => {
    if (!selectedPackage) return [];
    const warnings = [...selectedPackage.warnings];
    const currentVersion = otaSnapshot?.device.firmwareVersion;
    if (currentVersion && compareVersionish(selectedPackage.manifest.version, currentVersion) < 0) {
      warnings.push('Package version is older than the connected firmware version.');
    }
    return [...new Set(warnings)];
  }, [otaSnapshot?.device.firmwareVersion, selectedPackage]);
  const canStart = !!selectedPackage
    && state.userState === 'ready'
    && effectiveBlockers.length === 0
    && !transferActive
    && !preflightPending;

  const choosePackage = async (directory: boolean) => {
    const previousPackage = selectedPackage;
    setValidationErrors([]);
    setBlockers([]);
    setProgressBytes(null);
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        multiple: false,
        directory,
        filters: directory ? undefined : [{ name: 'Device firmware package', extensions: ['zip'] }],
      });
      if (typeof selected !== 'string') return;
      dispatch({ type: 'check' });
      const payload = await loadFirmwareOtaPackage(selected);
      const accepted = await acceptPackage(
        selected,
        payload.manifestText,
        new Uint8Array(payload.firmwareBytes),
        payload.sourceLabel,
        directory ? 'directory' : 'zip',
      );
      if (!accepted && previousPackage) {
        dispatch({ type: 'ready' });
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setValidationErrors([message]);
      dispatch(previousPackage
        ? { type: 'ready' }
        : { type: 'failed', failureCode: 'manifestMismatch', message });
    }
  };

  const acceptPackage = async (
    path: string,
    manifestText: string,
    firmwareBytes: Uint8Array,
    sourceLabel: string,
    sourceKind: SelectedPackage['sourceKind'],
  ): Promise<boolean> => {
    const result = await validateFirmwareOtaPackage(manifestText, firmwareBytes, {
      desktopVersion: APP_VERSION,
      expectedHardwareRevision: EXPECTED_HARDWARE_REVISION,
    });
    if (!result.ok || !result.manifest || !result.firmwareSha256) {
      dispatch({ type: 'failed', failureCode: result.errors.some(error => error.includes('SHA256')) ? 'hashFailure' : 'manifestMismatch', message: result.errors[0] ?? 'Invalid OTA package.' });
      setValidationErrors(result.errors);
      return false;
    }
    setSelectedPackage({
      path,
      manifest: result.manifest,
      manifestText,
      firmwareBytes,
      firmwareSha256: result.firmwareSha256,
      warnings: result.warnings,
      sourceLabel,
      sourceKind,
    });
    setOtaSnapshot(null);
    setOtaSnapshotFetchedAtMs(null);
    setSnapshotError(null);
    void refreshOtaSnapshot({ protocolName: result.manifest.protocolName });
    dispatch({ type: 'ready' });
    return true;
  };

  const startUpdate = async () => {
    const packageForUpdate = selectedPackage;
    if (!packageForUpdate || otaStartInFlightRef.current || transferActive) return;
    otaStartInFlightRef.current = true;
    let bytesSentForFailureCheck = 0;
    try {
      const snapshot = getFreshOtaSnapshot() ?? await refreshOtaSnapshot({ protocolName: packageForUpdate.manifest.protocolName });
      const check = evaluateFirmwareOtaPreflight({
        manifest: packageForUpdate.manifest,
        desktopVersion: APP_VERSION,
        recordingActive: snapshot.recordingActive,
        transferActive,
        device: snapshot.device,
      });
      if (!check.ok) {
        setBlockers(check.blockers);
        dispatch({ type: 'failed', failureCode: 'deviceRejected', message: '' });
        return;
      }

      setBlockers([]);
      dispatch({ type: 'startTransfer' });
      setProgressBytes({ sent: 0, total: packageForUpdate.firmwareBytes.byteLength });
      setTransferSpeedKibPerSec(null);
      transferSpeedSamplesRef.current = [{ tMs: performance.now(), bytes: 0 }];
      let transferResult: Awaited<ReturnType<typeof transferFirmwareOtaBle>> | null = null;
      const unlisten = await listen<{ bytesSent: number; bytesTotal: number }>('firmware-ota:progress', event => {
        const bytesTotal = Math.max(1, event.payload.bytesTotal);
        const bytesSent = Math.min(event.payload.bytesSent, bytesTotal);
        bytesSentForFailureCheck = Math.max(bytesSentForFailureCheck, bytesSent);
        setProgressBytes({ sent: bytesSent, total: bytesTotal });
        const nowMs = performance.now();
        const samples = transferSpeedSamplesRef.current;
        // Overall average from transfer start (t0 @ 0 bytes) → current offset.
        // Keep the origin sample + latest sample only; no rolling window.
        samples.push({ tMs: nowMs, bytes: bytesSent });
        if (samples.length > 2) {
          samples.splice(1, samples.length - 2);
        }
        const speed = estimateFirmwareOtaTransferSpeedKibPerSec(samples);
        if (speed != null) {
          setTransferSpeedKibPerSec(speed);
        }
        const pct = Math.round((bytesSent / bytesTotal) * 100);
        if (bytesSent >= bytesTotal) {
          dispatch({ type: 'transferComplete' });
        } else {
          dispatch({ type: 'transferProgress', progress: Math.min(pct, 99) });
        }
      });
      try {
        transferResult = await transferFirmwareOtaBle({
          manifest: packageForUpdate.manifest,
          firmwareBytes: packageForUpdate.firmwareBytes,
          expectedSha256: packageForUpdate.firmwareSha256,
        });
      } finally {
        unlisten();
      }
      dispatch({ type: 'transferComplete' });
      await delay(450);
      dispatch({ type: 'deviceReconnected' });
      await delay(450);
      if (firmwareOtaConfirmedVersionMatches(transferResult?.confirmedVersion, packageForUpdate.manifest.version)) {
        const confirmedVersion = transferResult?.confirmedVersion?.trim() ?? null;
        if (confirmedVersion) {
          setOtaSnapshot(previous => snapshotWithFirmwareVersion(previous, confirmedVersion));
        }
        dispatch({ type: 'verified' });
        void refreshOtaSnapshot({ protocolName: packageForUpdate.manifest.protocolName });
      } else {
        const confirmedVersion = transferResult?.confirmedVersion?.trim();
        dispatch(firmwareOtaVersionNotConfirmedAction(confirmedVersion, packageForUpdate.manifest.version));
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const expectedVersion = packageForUpdate.manifest.version;
      const rollbackVersionFromError = firmwareOtaRollbackVersionFromText(message, expectedVersion);
      if (rollbackVersionFromError) {
        setOtaSnapshot(previous => snapshotWithFirmwareVersion(previous, rollbackVersionFromError));
        dispatch(firmwareOtaVersionNotConfirmedAction(rollbackVersionFromError, expectedVersion));
        return;
      }
      const failedAfterFullTransfer = bytesSentForFailureCheck >= packageForUpdate.firmwareBytes.byteLength;
      if (failedAfterFullTransfer || looksLikePostRebootOtaError(message)) {
        const snapshotAfterFailure = await refreshOtaSnapshot({ waitForFirmwareVersion: true, protocolName: packageForUpdate.manifest.protocolName });
        const confirmedVersion = snapshotAfterFailure.device.firmwareVersion?.trim() ?? null;
        if (firmwareOtaConfirmedVersionLooksRolledBack(confirmedVersion, expectedVersion)) {
          if (confirmedVersion) {
            setOtaSnapshot(previous => snapshotWithFirmwareVersion(previous, confirmedVersion));
          }
          dispatch(firmwareOtaVersionNotConfirmedAction(confirmedVersion, expectedVersion));
          return;
        }
      }
      dispatch({
        type: 'failed',
        failureCode: message.toLowerCase().includes('disconnect') ? 'bleDisconnected' : 'deviceRejected',
        message,
      });
    } finally {
      otaStartInFlightRef.current = false;
      setProgressBytes(null);
      setTransferSpeedKibPerSec(null);
      transferSpeedSamplesRef.current = [];
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
      await exportDiagnosticPackage('listener-type-ota-diagnostics-device.zip');
      setDiagnosticStatus('ok');
    } catch (error) {
      console.warn('[firmware-ota] diagnostic export failed', error);
      setDiagnosticStatus('err');
    }
  };

  const startSelectedFirmwareAction = () => {
    if (firmwareMode === 'ble') {
      void startUpdate();
      return;
    }
    void wiredRef.current?.flash();
  };
  const selectedFirmwareActionDisabled = firmwareMode === 'ble' ? !canStart : !wiredAction.canFlash;
  const selectedFirmwareActionBusy = firmwareMode === 'ble' ? transferActive : wiredAction.isFlashing;
  const selectedFirmwareActionLabel = selectedFirmwareActionBusy
    ? firmwareMode === 'ble'
      ? statusLabel
      : t('settings.recording.wiredFirmwareFlashing', '刷入中')
    : t('settings.recording.firmwareStartSelected', '开始刷入');
  const selectedPackageDisplayName = selectedPackage
    ? formatFirmwarePackageDisplayName(selectedPackage.sourceLabel || selectedPackage.path)
    : '';
  const selectedPackageAriaLabel = selectedPackage
    ? t(
      'settings.recording.firmwareOtaReselectPackage',
      '重新选择固件包 {{name}}',
      { name: selectedPackageDisplayName },
    )
    : undefined;

  return (
    <>
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
      <div className="ol-firmware-ota-header" style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('settings.recording.firmwareOtaTitle', '设备固件')}
          </div>
        </div>
        <div className="ol-firmware-ota-actions" style={{ display: 'flex', gap: 8, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
          {selectedPackage ? (
            <button
              type="button"
              className="ol-firmware-selected-package"
              onClick={firmwareActionBusy ? undefined : () => void choosePackage(selectedPackage.sourceKind === 'directory')}
              disabled={firmwareActionBusy}
              aria-label={selectedPackageAriaLabel}
              title={selectedPackage.path}
            >
              <span className="ol-firmware-selected-package-icon">
                <Icon name="archive" size={16} />
              </span>
              <span className="ol-firmware-selected-package-copy">
                <span className="ol-firmware-selected-package-kicker">
                  {t('settings.recording.firmwareSelectedPackage', '已选固件')}
                </span>
                <span className="ol-firmware-selected-package-name">{selectedPackageDisplayName}</span>
              </span>
              <span className="ol-firmware-selected-package-meta">
                v{selectedPackage.manifest.version} · {formatBytes(selectedPackage.manifest.fileSizeBytes)}
                <Icon name="refresh" size={12} />
              </span>
            </button>
          ) : (
            <Btn variant="ghost" size="sm" icon="doc" onClick={() => void choosePackage(false)} disabled={firmwareActionBusy}>
              {t('settings.recording.firmwareOtaChoosePackage', '选择固件 zip')}
            </Btn>
          )}
          {!selectedPackage && (
            <Btn variant="soft" size="sm" icon="archive" onClick={() => void choosePackage(true)} disabled={firmwareActionBusy}>
              {t('settings.recording.firmwareOtaChoosePackageDir', '目录')}
            </Btn>
          )}
        </div>
      </div>

      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
        {t(
          'settings.recording.firmwareOtaDesc',
          '选择 firmware repo 生成的同一个固件发布包，然后选择蓝牙 OTA 或有线刷机。',
        )}
      </div>

      <div className="ol-firmware-control-bar">
        <div className="ol-firmware-mode-switch" style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
          <Btn variant={firmwareMode === 'ble' ? 'blue' : 'soft'} size="sm" icon="cloud" onClick={() => setFirmwareMode('ble')} disabled={firmwareActionBusy}>
            {t('settings.recording.firmwareModeBleOta', '蓝牙 OTA')}
          </Btn>
          <Btn variant={firmwareMode === 'wired' ? 'blue' : 'soft'} size="sm" icon="bolt" onClick={() => setFirmwareMode('wired')} disabled={firmwareActionBusy}>
            {t('settings.recording.firmwareModeWired', '有线刷机')}
          </Btn>
        </div>

        <div className="ol-firmware-unified-actions" style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center', justifyContent: 'flex-end' }}>
          <Btn variant="blue" size="sm" icon={firmwareMode === 'wired' ? 'bolt' : 'cloud'} onClick={startSelectedFirmwareAction} disabled={selectedFirmwareActionDisabled}>
            {selectedFirmwareActionLabel}
          </Btn>
        </div>
      </div>

      {firmwareMode === 'ble' && (
        <div
          style={{
            paddingTop: 12,
            borderTop: '0.5px solid var(--ol-line-soft)',
            display: 'flex',
            flexDirection: 'column',
            gap: 12,
          }}
        >
          <div className="ol-firmware-ble-header" style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
            <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--ol-ink)' }}>
              {t('settings.recording.firmwareModeBleOta', '蓝牙 OTA')}
            </div>
            <Pill tone={statusTone} size="sm">{statusLabel}</Pill>
          </div>

          <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
            {t(
              'settings.recording.firmwareOtaBleDesc',
              '通过蓝牙发送 OTA 固件，只读取 ota_manifest.json 和 firmware_ota.bin，不占用 BLE audio 或 HID。',
            )}
          </div>

          {!selectedPackage && (
            <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
              {t('settings.recording.firmwareOtaNeedsSharedPackage', '先在上方选择固件发布包，再选择蓝牙 OTA。')}
            </div>
          )}

          {selectedPackage && (
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(156px, 1fr))', gap: 8 }}>
              <FirmwareOtaFact label={t('settings.recording.firmwareOtaPackageVersion', '升级包版本')} value={selectedPackage.manifest.version} />
              <FirmwareOtaFact label={t('settings.recording.firmwareOtaChannel', '渠道')} value={selectedPackage.manifest.channel} />
              <FirmwareOtaFact label={t('settings.recording.firmwareOtaSize', '升级包大小')} value={formatBytes(selectedPackage.manifest.fileSizeBytes)} />
            </div>
          )}

          {selectedPackage && (
            <FirmwareOtaReadinessSummary
              snapshot={otaSnapshot}
              snapshotError={snapshotError}
              refreshing={snapshotRefreshing}
              onRefresh={() => void refreshOtaSnapshot({ waitForFirmwareVersion: true, protocolName: selectedPackage?.manifest.protocolName ?? null })}
              t={t}
            />
          )}

          {(state.userState === 'transferring' || state.userState === 'rebooting' || state.userState === 'verifying') && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 10 }}>
                <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', lineHeight: 1.4 }}>
                  {state.userState === 'transferring'
                    ? t('settings.recording.firmwareOtaTransferProgress', '正在发送固件...')
                    : state.userState === 'rebooting'
                      ? t('settings.recording.firmwareOtaFinalizeProgress', '固件已发送，正在校验并准备重启...')
                      : t('settings.recording.firmwareOtaVerifyProgress', '正在重新连接并确认固件版本...')}
                </div>
                {state.userState === 'transferring' && (
                  <span
                    style={{ fontSize: 11, color: 'var(--ol-ink-3)', fontVariantNumeric: 'tabular-nums', whiteSpace: 'nowrap' }}
                    title={t('settings.recording.firmwareOtaTransferSpeedHint', '约等于最近几秒的平均传输速度（协议吞吐，供参考）')}
                  >
                    {transferSpeedKibPerSec != null
                      ? t(
                          'settings.recording.firmwareOtaTransferSpeed',
                          '平均 {{speed}} KiB/s',
                          {
                            speed: formatFirmwareOtaTransferSpeed(transferSpeedKibPerSec),
                          },
                        )
                      : t('settings.recording.firmwareOtaTransferSpeedPending', '测速中…')}
                  </span>
                )}
              </div>
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
                <span style={{ fontSize: 11, color: 'var(--ol-ink-4)', minWidth: 112, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {state.userState === 'transferring' && progressBytes
                    ? `${formatBytes(progressBytes.sent)} / ${formatBytes(progressBytes.total)}`
                    : state.userState === 'transferring' ? `${state.progress}%` : t('settings.recording.firmwareOtaFinalizingBytes', '已传完')}
                </span>
              </div>
            </div>
          )}

          {(validationErrors.length > 0 || visiblePreflightBlockers.length > 0 || state.failureCode) && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {validationErrors.map(error => (
                <div key={error} style={{ fontSize: 11.5, color: 'var(--ol-err)', lineHeight: 1.5 }}>
                  {error}
                </div>
              ))}
              {visiblePreflightBlockers.map(item => (
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
                    {formatFirmwareOtaFailureNextStep(
                      state.failureCode,
                      i18n.resolvedLanguage ?? i18n.language,
                      state.message,
                    )}
                  </div>
                </>
              )}
            </div>
          )}

          {dynamicWarnings.map(warning => (
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
      )}
      {firmwareMode === 'wired' && (
        <FirmwareWiredFlashPanel ref={wiredRef} packagePath={selectedPackage?.path ?? null} disabled={transferActive} onBusyChange={setWiredBusy} onActionStateChange={setWiredAction} />
      )}
    </div>
    </>
  );
}

interface WiredPackageSelection {
  path: string;
  payload: WiredFirmwarePackagePayload;
}

type WiredFlashStatus = 'idle' | 'checking' | 'ready' | 'flashing' | 'ok' | 'err';

interface FirmwareWiredFlashPanelProps {
  packagePath: string | null;
  disabled?: boolean;
  onBusyChange?: (busy: boolean) => void;
  onActionStateChange?: (state: { canFlash: boolean; isFlashing: boolean }) => void;
}

interface FirmwareWiredFlashHandle {
  flash: () => void;
}

const FirmwareWiredFlashPanel = forwardRef<FirmwareWiredFlashHandle, FirmwareWiredFlashPanelProps>(function FirmwareWiredFlashPanel({
  packagePath,
  disabled = false,
  onBusyChange,
  onActionStateChange,
}, ref) {
  const { t } = useTranslation();
  const [selection, setSelection] = useState<WiredPackageSelection | null>(null);
  const [ports, setPorts] = useState<WiredFirmwareSerialPort[]>([]);
  const [port, setPort] = useState('COMx');
  const [baud, setBaud] = useState('460800');
  const [preserveOtaData, setPreserveOtaData] = useState(false);
  const [status, setStatus] = useState<WiredFlashStatus>('idle');
  const [message, setMessage] = useState<string | null>(null);
  const [result, setResult] = useState<WiredFirmwareFlashResult | null>(null);
  const [progress, setProgress] = useState<WiredFirmwareProgressPayload | null>(null);

  const selectedPayload = selection?.payload ?? null;
  const activeWiredPort = port;
  const activeWiredBaud = parseBaud(baud) ?? 460800;
  const activePreserveOtaData = preserveOtaData;
  const busy = status === 'checking' || status === 'flashing';
  const statusTone = wiredStatusTone(status);
  const statusLabel = wiredStatusLabel(status, t);
  const canFlash = !!selection && !busy && !disabled;
  const isFlashing = status === 'flashing';

  useEffect(() => {
    onBusyChange?.(busy);
  }, [busy, onBusyChange]);

  useEffect(() => {
    return () => onBusyChange?.(false);
  }, [onBusyChange]);

  useEffect(() => {
    onActionStateChange?.({ canFlash, isFlashing });
  }, [canFlash, isFlashing, onActionStateChange]);

  useEffect(() => {
    return () => onActionStateChange?.({ canFlash: false, isFlashing: false });
  }, [onActionStateChange]);

  const refreshPorts = useCallback(async () => {
    try {
      const nextPorts = await listWiredFirmwarePorts();
      setPorts(nextPorts);
      const likely = nextPorts.find(item => item.isLikelyEsp32);
      if (likely && (port === 'COMx' || port.trim() === '')) {
        setPort(likely.port);
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  }, [port]);

  useEffect(() => {
    void refreshPorts();
  }, [refreshPorts]);

  useEffect(() => {
    let cancelled = false;
    setSelection(null);
    setResult(null);
    setMessage(null);
    setProgress(null);
    if (!packagePath) {
      setStatus('idle');
      return () => {
        cancelled = true;
      };
    }
    setStatus('checking');
    void loadWiredFirmwarePackage(packagePath)
      .then(payload => {
        if (cancelled) return;
        setSelection({ path: packagePath, payload });
        setStatus('ready');
      })
      .catch(error => {
        if (cancelled) return;
        setStatus('err');
        setMessage(error instanceof Error ? error.message : String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [packagePath]);

  const runWiredOperationWithProgress = async (
    action: WiredFirmwareProgressPayload['action'],
    operation: () => Promise<WiredFirmwareFlashResult>,
  ) => {
    setProgress(makeInitialWiredProgress(action, activeWiredPort, selection?.payload.version ?? null));
    const unlisten = await listen<WiredFirmwareProgressPayload>('wired-firmware:progress', event => {
      if (event.payload.action === action) {
        setProgress(event.payload);
      }
    });
    try {
      return await operation();
    } finally {
      unlisten();
    }
  };

  const startWiredFlash = async () => {
    if (!selection) return;
    setStatus('flashing');
    setMessage(null);
    setResult(null);
    try {
      const nextResult = await runWiredOperationWithProgress('flash', () =>
        flashWiredFirmwarePackage({
          path: selection.path,
          port: activeWiredPort,
          baud: activeWiredBaud,
          preserveOtaData: activePreserveOtaData,
        }),
      );
      setResult(nextResult);
      setProgress(makeDoneWiredProgress('flash', nextResult.port, nextResult.version));
      setStatus('ok');
    } catch (error) {
      setStatus('err');
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  useImperativeHandle(ref, () => ({
    flash: () => {
      void startWiredFlash();
    },
  }), [startWiredFlash]);

  const artifacts = selectedPayload?.artifacts ?? [];

  return (
    <div
      style={{
        paddingTop: 12,
        borderTop: '0.5px solid var(--ol-line-soft)',
        display: 'flex',
        flexDirection: 'column',
        gap: 12,
      }}
    >
      <div className="ol-firmware-wired-header" style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('settings.recording.wiredFirmwareTitle', '有线出厂刷机')}
          </div>
          <Pill tone={statusTone} size="sm">{statusLabel}</Pill>
        </div>
        <div className="ol-firmware-wired-actions" style={{ display: 'flex', gap: 8, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
          <Btn variant="soft" size="sm" icon="refresh" onClick={() => void refreshPorts()} disabled={busy || disabled}>
            {t('settings.recording.wiredFirmwareRefreshPorts', '串口')}
          </Btn>
        </div>
      </div>

      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
        {t(
          'settings.recording.wiredFirmwareDesc',
          '使用上方已选择的同一个固件发布包，通过 USB/串口读取 factory 子包。按下刷入时会自动检查 0x0 是否已有 bootloader：没有或损坏则随全量刷写一并修复，并写入分区表和 app。无需单独的 Boot 修复按钮。',
        )}
      </div>

      {!packagePath && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
          {t('settings.recording.wiredFirmwareNeedsSharedPackage', '先在上方选择固件发布包，再选择有线刷机。')}
        </div>
      )}

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(140px, 1fr))', gap: 8 }}>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 4, minWidth: 0 }}>
          <span style={{ fontSize: 10.5, color: 'var(--ol-ink-4)' }}>{t('settings.recording.wiredFirmwarePort', '串口')}</span>
          <input
            list="listener-wired-firmware-ports"
            value={port}
            onChange={event => setPort(event.target.value)}
            placeholder="COMx"
            disabled={busy || disabled || !packagePath}
            style={wiredInputStyle}
          />
          <datalist id="listener-wired-firmware-ports">
            <option value="COMx">{t('settings.recording.wiredFirmwareAutoPort', '自动识别')}</option>
            {ports.map(item => <option key={item.port} value={item.port}>{item.label}</option>)}
          </datalist>
        </label>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 4, minWidth: 0 }}>
          <span style={{ fontSize: 10.5, color: 'var(--ol-ink-4)' }}>{t('settings.recording.wiredFirmwareBaud', '刷机波特率')}</span>
          <input
            value={baud}
            onChange={event => setBaud(event.target.value.replace(/[^\d]/g, '').slice(0, 7))}
            placeholder="460800"
            disabled={busy || disabled || !packagePath}
            inputMode="numeric"
            style={wiredInputStyle}
          />
        </label>
        <label style={{ display: 'flex', alignItems: 'center', gap: 8, paddingTop: 18, minWidth: 0, fontSize: 11.5, color: 'var(--ol-ink-3)' }}>
          <input
            type="checkbox"
            checked={preserveOtaData}
            onChange={event => setPreserveOtaData(event.target.checked)}
            disabled={busy || disabled || !packagePath}
          />
          <span>{t('settings.recording.wiredFirmwarePreserveOta', '保留 OTA 选择区')}</span>
        </label>
      </div>

      {selectedPayload && (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(156px, 1fr))', gap: 8 }}>
          <FirmwareOtaFact label={t('settings.recording.wiredFirmwarePackageType', '包类型')} value="factory full flash" />
          <FirmwareOtaFact label={t('settings.recording.wiredFirmwareVersion', '版本')} value={selectedPayload.version} />
          <FirmwareOtaFact label={t('settings.recording.wiredFirmwareTarget', '芯片')} value={selectedPayload.target} />
        </div>
      )}

      {artifacts.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          {artifacts.map(item => (
            <div key={`${item.role}-${item.file}`} style={{ fontSize: 11, color: 'var(--ol-ink-4)', fontFamily: 'var(--ol-font-mono)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {item.role}@{item.offset} {item.file} {item.sizeBytes > 0 ? formatBytes(item.sizeBytes) : ''}
            </div>
          ))}
        </div>
      )}

      {selectedPayload?.notes.map(note => (
        <div key={note} style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
          {note}
        </div>
      ))}

      {progress && (busy || status === 'ok' || status === 'err') && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <div style={{ flex: 1, height: 6, borderRadius: 999, overflow: 'hidden', background: 'var(--ol-control-track)' }}>
              <div
                style={{
                  width: `${Math.max(2, Math.min(100, progress.percent))}%`,
                  height: '100%',
                  background: progress.action === 'bootloaderRepair' ? 'var(--ol-warn)' : 'var(--ol-blue)',
                  transition: 'width 0.25s ease',
                }}
              />
            </div>
            <span style={{ fontSize: 11, color: 'var(--ol-ink-4)', minWidth: 112, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
              {progress.bytesTotal > 0
                ? `${formatBytes(progress.bytesWritten)} / ${formatBytes(progress.bytesTotal)}`
                : `${Math.max(0, Math.min(100, progress.percent))}%`}
            </span>
          </div>
          <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', lineHeight: 1.4 }}>
            {formatWiredProgressMessage(progress, t)}
          </div>
        </div>
      )}

      {message && (
        <div style={{ fontSize: 11.5, color: status === 'err' ? 'var(--ol-err)' : 'var(--ol-ink-4)', lineHeight: 1.5 }}>
          {message}
        </div>
      )}

      {result && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          <div style={{ fontSize: 11.5, color: 'var(--ol-ok)', lineHeight: 1.5 }}>
            {result.action === 'bootloaderRepair'
              ? t('settings.recording.wiredFirmwareRepairDone', 'Boot 修复完成。')
              : t('settings.recording.wiredFirmwareFlashDone', '有线刷机完成。')}
          </div>
          <pre style={{ margin: 0, maxHeight: 156, overflow: 'auto', padding: 10, borderRadius: 7, background: 'var(--ol-control-track)', color: 'var(--ol-ink-3)', fontSize: 10.5, lineHeight: 1.45, whiteSpace: 'pre-wrap' }}>
            {result.log}
          </pre>
        </div>
      )}
    </div>
  );
});

function makeInitialWiredProgress(
  action: WiredFirmwareProgressPayload['action'],
  port: string,
  version: string | null,
): WiredFirmwareProgressPayload {
  return {
    action,
    stage: 'loading',
    port: port && port !== 'COMx' ? port : null,
    version,
    currentRole: null,
    currentFile: null,
    bytesWritten: 0,
    bytesTotal: 0,
    currentBytes: 0,
    currentTotal: 0,
    percent: 1,
    message: 'Loading wired firmware package',
  };
}

function makeDoneWiredProgress(
  action: WiredFirmwareProgressPayload['action'],
  port: string,
  version: string,
): WiredFirmwareProgressPayload {
  return {
    action,
    stage: 'done',
    port,
    version,
    currentRole: null,
    currentFile: null,
    bytesWritten: 0,
    bytesTotal: 0,
    currentBytes: 0,
    currentTotal: 0,
    percent: 100,
    message: action === 'bootloaderRepair' ? 'Bootloader repair completed' : 'Wired factory flash completed',
  };
}

function formatWiredProgressMessage(
  progress: WiredFirmwareProgressPayload,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  switch (progress.stage) {
    case 'loading':
      return t('settings.recording.wiredFirmwareProgressLoading', '正在读取固件包...');
    case 'packageLoaded':
      if (progress.port === 'SWD') {
        return t('settings.recording.wiredFirmwareProgressPackageLoadedSwd', '固件包已读取，正在准备 ST-LINK/SWD。');
      }
      return t('settings.recording.wiredFirmwareProgressPackageLoaded', '固件包已读取，正在准备串口。');
    case 'connecting':
      if (progress.port === 'SWD') {
        return t('settings.recording.wiredFirmwareProgressConnectingSwd', '正在连接 ST-LINK/SWD。');
      }
      return progress.action === 'bootloaderRepair'
        ? t('settings.recording.wiredFirmwareProgressRepairConnecting', '正在等待 Boot 修复窗口并连接串口...')
        : t('settings.recording.wiredFirmwareProgressConnecting', '正在连接串口刷机模式...');
    case 'connected':
      return progress.port
        ? t('settings.recording.wiredFirmwareProgressConnectedPort', '已连接 {{port}}。', { port: progress.port })
        : t('settings.recording.wiredFirmwareProgressConnected', '已连接设备。');
    case 'erasing':
      return t('settings.recording.wiredFirmwareProgressErasing', '正在擦除 OTA 状态区...');
    case 'erased':
      return t('settings.recording.wiredFirmwareProgressErased', 'OTA 状态区已处理。');
    case 'preparing':
      return t('settings.recording.wiredFirmwareProgressPreparing', '正在准备刷机镜像...');
    case 'writing': {
      const role = wiredArtifactRoleLabel(progress.currentRole, t);
      const file = progress.currentFile ? ` ${progress.currentFile}` : '';
      const bytes = progress.currentTotal > 0
        ? ` ${formatBytes(progress.currentBytes)} / ${formatBytes(progress.currentTotal)}`
        : '';
      return t('settings.recording.wiredFirmwareProgressWriting', '正在写入 {{role}}{{file}}...{{bytes}}', {
        role,
        file,
        bytes,
      });
    }
    case 'finalizing':
      return t('settings.recording.wiredFirmwareProgressFinalizing', '正在校验并收尾...');
    case 'done':
      return progress.action === 'bootloaderRepair'
        ? t('settings.recording.wiredFirmwareProgressRepairDone', 'Boot 修复完成。')
        : t('settings.recording.wiredFirmwareProgressDone', '有线刷机完成。');
    default:
      return progress.message || `${progress.percent}%`;
  }
}

function wiredArtifactRoleLabel(
  role: string | null,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  switch (role) {
    case 'bootloader':
      return t('settings.recording.wiredFirmwareRoleBootloader', 'bootloader');
    case 'partition_table':
      return t('settings.recording.wiredFirmwareRolePartitionTable', '分区表');
    case 'app':
      return t('settings.recording.wiredFirmwareRoleApp', 'app');
    default:
      return role || t('settings.recording.wiredFirmwareRoleFirmware', '固件');
  }
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
  const statusPending = refreshing && !snapshot;
  const otaServiceConfirmed = !!device?.connected && device.capabilities.includes(LISTENER_OTA_V1_TRANSPORT_BOUNDARY.protocolName);
  const rows: Array<[string, string]> = [
    [t('settings.recording.firmwareOtaDeviceFirmware', '固件'), statusPending ? t('settings.recording.firmwareOtaReading', '读取中') : device?.firmwareVersion ?? (otaServiceConfirmed ? t('settings.recording.firmwareOtaVersionNotReported', '版本未报告') : t('settings.recording.firmwareOtaUnavailable', '未获取'))],
    [t('settings.recording.firmwareOtaDeviceConnected', '连接'), statusPending ? t('settings.recording.firmwareOtaReading', '读取中') : device?.connected ? t('settings.recording.firmwareOtaConnected', '已连接') : t('settings.recording.firmwareOtaDisconnected', '未连接')],
    [t('settings.recording.firmwareOtaDeviceHardware', '硬件'), statusPending ? t('settings.recording.firmwareOtaReading', '读取中') : device?.hardwareRevision ?? (otaServiceConfirmed ? t('settings.recording.firmwareOtaCompatibleHardware', '兼容') : t('settings.recording.firmwareOtaUnavailable', '未获取'))],
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
      {snapshotError && !refreshing && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
          {formatFirmwareOtaSnapshotError(snapshotError, t)}
        </div>
      )}
    </div>
  );
}

function formatFirmwareOtaSnapshotError(
  error: string,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (/timeout|timed out|超时/i.test(error)) {
    return t('settings.recording.firmwareOtaSnapshotTimeout', '设备状态暂时没有响应，请重新查询后再更新。');
  }
  return t('settings.recording.firmwareOtaSnapshotUnavailable', '暂时无法读取设备状态，请确认蓝牙连接后重新查询。');
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

function makeActiveFirmwareOtaSnapshot(): FirmwareOtaPreflightSnapshot {
  return {
    recordingActive: false,
    dictationPhase: 'firmware_ota',
    device: {
      connected: true,
      hardwareRevision: null,
      firmwareVersion: null,
      capabilities: ['firmware_ota_transfer_active'],
      batteryPercent: null,
      usbPowered: null,
      detail: 'Firmware OTA is in progress; device polling is paused until transfer completes.',
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

function looksLikePostRebootOtaError(message: string): boolean {
  const text = message.toLowerCase();
  return (
    text.includes('finish') ||
    text.includes('reboot') ||
    text.includes('confirm') ||
    text.includes('version') ||
    text.includes('verify') ||
    text.includes('rollback')
  );
}

function formatBytes(bytes: number): string {
  const safeBytes = Math.max(0, bytes);
  const kib = safeBytes / 1024;
  if (kib < 1024) {
    return `${kib.toFixed(1)} KB`;
  }
  return `${(kib / 1024).toFixed(2)} MB`;
}

function formatFirmwarePackageDisplayName(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) return 'firmware package';
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : trimmed;
}

function parseBaud(value: string): number | null {
  const parsed = Number.parseInt(value.trim(), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

const wiredInputStyle: React.CSSProperties = {
  height: 30,
  minWidth: 0,
  borderRadius: 7,
  border: '0.5px solid var(--ol-line-soft)',
  background: 'var(--ol-control-track)',
  color: 'var(--ol-ink)',
  fontSize: 11.5,
  padding: '0 9px',
  fontFamily: 'var(--ol-font-mono)',
};

function wiredStatusTone(state: WiredFlashStatus): PillTone {
  switch (state) {
    case 'ready':
    case 'ok':
      return 'ok';
    case 'checking':
    case 'flashing':
      return 'blue';
    case 'err':
      return 'err';
    case 'idle':
      return 'outline';
  }
}

function wiredStatusLabel(state: WiredFlashStatus, t: ReturnType<typeof useTranslation>['t']): string {
  switch (state) {
    case 'idle':
      return t('settings.recording.wiredFirmwareIdle', '未选择');
    case 'checking':
      return t('settings.recording.wiredFirmwareChecking', '检查中');
    case 'ready':
      return t('settings.recording.wiredFirmwareReady', '可刷入');
    case 'flashing':
      return t('settings.recording.wiredFirmwareFlashing', '刷入中');
    case 'ok':
      return t('settings.recording.wiredFirmwareOk', '完成');
    case 'err':
      return t('settings.recording.wiredFirmwareErr', '失败');
  }
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
    case 'downgrade':
      return localizeOtaText(language, '升级包版本低于当前设备固件；同版本可重刷，回退请用 USB factory 恢复。', 'The OTA package is older than the device firmware; same-version reflashing is allowed, but rollback should use USB factory recovery.');
  }
}

function formatFirmwareOtaFailureNextStep(
  code: NonNullable<ReturnType<typeof firmwareOtaReducer>['failureCode']>,
  language: string,
  detailMessage = '',
): string {
  const detail = detailMessage.toLowerCase();
  switch (code) {
    case 'bleDisconnected':
      return localizeOtaText(language, '请重新连接 Listener，查询设备状态后再试。', 'Reconnect Listener, query device status, then retry.');
    case 'manifestMismatch':
      return localizeOtaText(language, '请重新选择同一个 OTA 包中的 manifest 和固件文件。', 'Choose the manifest and firmware from the same OTA package.');
    case 'hashFailure':
      return localizeOtaText(language, '升级包校验失败，请重新生成或下载 OTA 包。', 'OTA package verification failed. Rebuild or download the package again.');
    case 'deviceRejected':
      // Catch-all bucket: many transport failures (timeout / begin / WWR) land here.
      // Prefer actionable copy from the real host error; keep raw message above.
      if (
        detail.includes('timed out')
        || detail.includes('timeout')
        || detail.includes('超时')
      ) {
        return localizeOtaText(
          language,
          '传输超时：请保持设备唤醒与 USB 供电，暂停说话/录音后重试。完整原因见上方英文错误与 Type 日志 [firmware-ota] transfer failed。',
          'Transfer timed out. Keep the device awake on USB power, stop speech/recording, then retry. Full reason is above and in Type log [firmware-ota] transfer failed.',
        );
      }
      if (
        detail.includes('begin')
        || detail.includes('protocol_error')
        || detail.includes('authentication')
        || detail.includes('insufficient')
      ) {
        return localizeOtaText(
          language,
          'BEGIN/加密会话失败：可先断电复位设备，确认 Type 已接上后再试；串口看 firmware_ota / ble_firmware_ota 是否出现 begin rejected。',
          'BEGIN/encryption session failed. Power-cycle the device, ensure Type is ready, then retry. Check serial for firmware_ota / ble_firmware_ota begin rejected.',
        );
      }
      if (detail.trim().length > 0) {
        return localizeOtaText(
          language,
          '上方为 Type 返回的完整错误。请对照 %LOCALAPPDATA%\\Listener Type\\Logs\\listener-type.log 中 [firmware-ota] transfer failed 与设备串口 OTA 日志。',
          'The full host error is shown above. Cross-check %LOCALAPPDATA%\\Listener Type\\Logs\\listener-type.log ([firmware-ota] transfer failed) and device serial OTA logs.',
        );
      }
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
  if (warning.includes('older than the connected firmware version')) {
    return localizeOtaText(language, '升级包版本低于当前设备固件；同版本 OTA 可直接重刷。', 'The OTA package is older than the device firmware; same-version OTA reflashing is allowed.');
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
      return t('settings.recording.firmwareOtaRebooting', '校验中');
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

function withTimeout<T>(promise: Promise<T>, timeoutMs: number, timeoutMessage: string): Promise<T> {
  let timer: number | undefined;
  const timeout = new Promise<T>((_, reject) => {
    timer = window.setTimeout(() => reject(new Error(timeoutMessage)), timeoutMs);
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timer !== undefined) {
      window.clearTimeout(timer);
    }
  });
}
