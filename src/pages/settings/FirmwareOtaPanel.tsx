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
  repairWiredFirmwareBootloader,
  transferFirmwareOtaBle,
  type WiredFirmwareFlashResult,
  type WiredFirmwarePackagePayload,
  type WiredFirmwareProgressPayload,
  type WiredFirmwareSerialPort,
} from '../../lib/ipc';
import {
  compareVersionish,
  evaluateFirmwareOtaPreflight,
  firmwareOtaConfirmedVersionMatches,
  firmwareOtaConfirmedVersionLooksRolledBack,
  firmwareOtaReducer,
  firmwareOtaRollbackVersionFromText,
  firmwareOtaVersionNotConfirmedAction,
  initialFirmwareOtaState,
  isCompanionOtaV2Manifest,
  isStm32wbStBleOtaManifest,
  validateFirmwareOtaPackage,
  type FirmwareOtaBlocker,
  type FirmwareOtaDeviceSnapshot,
  type FirmwareOtaManifest,
  type FirmwareOtaPreflightSnapshot,
  type FirmwareOtaUserState,
} from '../../lib/firmwareOta';
import { Btn, Pill, type PillTone } from '../_atoms';
import type { EmbeddedBleProbeStatus } from '../../components/EmbeddedBleStatusPanel';

const EXPECTED_HARDWARE_REVISION = 'keyboard-v2-n16r8';
const OTA_VERSION_QUERY_TIMEOUT_MS = 15_000;
const OTA_VERSION_QUERY_POLL_MS = 700;

interface SelectedPackage {
  path: string;
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
  const [state, dispatch] = useReducer(firmwareOtaReducer, initialFirmwareOtaState);
  const [firmwareMode, setFirmwareMode] = useState<'ble' | 'wired'>('ble');
  const [wiredBusy, setWiredBusy] = useState(false);
  const wiredRef = useRef<FirmwareWiredFlashHandle>(null);
  const [wiredAction, setWiredAction] = useState<{ canFlash: boolean; isFlashing: boolean }>({ canFlash: false, isFlashing: false });
  const [selectedPackage, setSelectedPackage] = useState<SelectedPackage | null>(null);
  const [blockers, setBlockers] = useState<FirmwareOtaBlocker[]>([]);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [otaSnapshot, setOtaSnapshot] = useState<FirmwareOtaPreflightSnapshot | null>(null);
  const [snapshotError, setSnapshotError] = useState<string | null>(null);
  const [snapshotRefreshing, setSnapshotRefreshing] = useState(false);
  const [diagnosticStatus, setDiagnosticStatus] = useState<'idle' | 'busy' | 'ok' | 'err'>('idle');
  const [progressBytes, setProgressBytes] = useState<{ sent: number; total: number } | null>(null);

  const transferActive = state.userState === 'transferring' || state.userState === 'rebooting' || state.userState === 'verifying';
  const firmwareActionBusy = transferActive || wiredBusy;
  const statusTone = userStateTone(state.userState);
  const statusLabel = userStateLabel(state.userState, t);

  const refreshOtaSnapshot = useCallback(async (options: { waitForFirmwareVersion?: boolean } = {}) => {
    const waitForFirmwareVersion = options.waitForFirmwareVersion ?? false;
    setSnapshotRefreshing(true);
    if (!supported) {
      const unsupported = makeDisconnectedSnapshot('Firmware OTA is only supported on Windows Listener BLE.');
      setOtaSnapshot(unsupported);
      setSnapshotError(null);
      setSnapshotRefreshing(false);
      return unsupported;
    }
    const deadline = Date.now() + OTA_VERSION_QUERY_TIMEOUT_MS;
    let lastSnapshot: FirmwareOtaPreflightSnapshot | null = null;
    let lastError: string | null = null;
    try {
      do {
        try {
          const snapshot = await getFirmwareOtaPreflightSnapshot();
          lastSnapshot = snapshot;
          setOtaSnapshot(snapshot);
          setSnapshotError(null);
          if (!waitForFirmwareVersion || snapshot.device.firmwareVersion) {
            return snapshot;
          }
        } catch (error) {
          lastError = error instanceof Error ? error.message : String(error);
          if (!waitForFirmwareVersion) {
            const failed = makeDisconnectedSnapshot(lastError);
            setOtaSnapshot(failed);
            setSnapshotError(lastError);
            return failed;
          }
        }
        await delay(OTA_VERSION_QUERY_POLL_MS);
      } while (Date.now() <= deadline);

      const timeoutMessage = t('settings.recording.firmwareOtaRefreshTimeout', '查询固件版本超时，请重试。');
      if (lastSnapshot) {
        setOtaSnapshot(lastSnapshot);
        setSnapshotError(timeoutMessage);
        return lastSnapshot;
      }
      const failed = makeDisconnectedSnapshot(lastError ?? timeoutMessage);
      setOtaSnapshot(failed);
      setSnapshotError(lastError ?? timeoutMessage);
      return failed;
    } finally {
      setSnapshotRefreshing(false);
    }
  }, [supported, t]);

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
  const dynamicWarnings = useMemo(() => {
    if (!selectedPackage) return [];
    const warnings = [...selectedPackage.warnings];
    const currentVersion = otaSnapshot?.device.firmwareVersion;
    if (currentVersion && compareVersionish(selectedPackage.manifest.version, currentVersion) <= 0) {
      warnings.push('Package version is not newer than the connected firmware version.');
    }
    return [...new Set(warnings)];
  }, [otaSnapshot?.device.firmwareVersion, selectedPackage]);
  const canStart = !!selectedPackage && state.userState === 'ready' && effectiveBlockers.length === 0 && !transferActive;

  const choosePackage = async (directory: boolean) => {
    setValidationErrors([]);
    setBlockers([]);
    setSelectedPackage(null);
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
      await acceptPackage(selected, payload.manifestText, new Uint8Array(payload.firmwareBytes), payload.sourceLabel);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      dispatch({ type: 'failed', failureCode: 'manifestMismatch', message });
      setValidationErrors([message]);
    }
  };

  const acceptPackage = async (path: string, manifestText: string, firmwareBytes: Uint8Array, sourceLabel: string) => {
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
      path,
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
      dispatch({ type: 'failed', failureCode: 'deviceRejected', message: '' });
      return;
    }

    setBlockers([]);
    let bytesSentForFailureCheck = 0;
    try {
      dispatch({ type: 'startTransfer' });
      setProgressBytes({ sent: 0, total: selectedPackage.firmwareBytes.byteLength });
      let transferResult: Awaited<ReturnType<typeof transferFirmwareOtaBle>> | null = null;
      const unlisten = await listen<{ bytesSent: number; bytesTotal: number }>('firmware-ota:progress', event => {
        const bytesTotal = Math.max(1, event.payload.bytesTotal);
        const bytesSent = Math.min(event.payload.bytesSent, bytesTotal);
        bytesSentForFailureCheck = Math.max(bytesSentForFailureCheck, bytesSent);
        setProgressBytes({ sent: bytesSent, total: bytesTotal });
        const pct = Math.round((bytesSent / bytesTotal) * 100);
        if (bytesSent >= bytesTotal) {
          dispatch({ type: 'transferComplete' });
        } else {
          dispatch({ type: 'transferProgress', progress: Math.min(pct, 99) });
        }
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
      if (
        firmwareOtaConfirmedVersionMatches(transferResult?.confirmedVersion, selectedPackage.manifest.version) ||
        ((isStm32wbStBleOtaManifest(selectedPackage.manifest) || isCompanionOtaV2Manifest(selectedPackage.manifest)) &&
          transferResult?.transport === selectedPackage.manifest.protocolName)
      ) {
        const confirmedVersion = transferResult?.confirmedVersion?.trim() ?? null;
        if (confirmedVersion) {
          setOtaSnapshot(previous => snapshotWithFirmwareVersion(previous, confirmedVersion));
        }
        dispatch({ type: 'verified' });
        void refreshOtaSnapshot();
      } else {
        const confirmedVersion = transferResult?.confirmedVersion?.trim();
        dispatch(firmwareOtaVersionNotConfirmedAction(confirmedVersion, selectedPackage.manifest.version));
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const expectedVersion = selectedPackage.manifest.version;
      const rollbackVersionFromError = firmwareOtaRollbackVersionFromText(message, expectedVersion);
      if (rollbackVersionFromError) {
        setOtaSnapshot(previous => snapshotWithFirmwareVersion(previous, rollbackVersionFromError));
        dispatch(firmwareOtaVersionNotConfirmedAction(rollbackVersionFromError, expectedVersion));
        return;
      }
      const failedAfterFullTransfer = bytesSentForFailureCheck >= selectedPackage.firmwareBytes.byteLength;
      if (failedAfterFullTransfer || looksLikePostRebootOtaError(message)) {
        const snapshotAfterFailure = await refreshOtaSnapshot({ waitForFirmwareVersion: true });
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
          <Btn variant="ghost" size="sm" icon="doc" onClick={() => void choosePackage(false)} disabled={firmwareActionBusy}>
            {t('settings.recording.firmwareOtaChoosePackage', '选择固件 zip')}
          </Btn>
          <Btn variant="soft" size="sm" icon="archive" onClick={() => void choosePackage(true)} disabled={firmwareActionBusy}>
            {t('settings.recording.firmwareOtaChoosePackageDir', '目录')}
          </Btn>
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
              onRefresh={() => void refreshOtaSnapshot({ waitForFirmwareVersion: true })}
              t={t}
            />
          )}

          {(state.userState === 'transferring' || state.userState === 'rebooting' || state.userState === 'verifying') && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
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
              <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', lineHeight: 1.4 }}>
                {state.userState === 'transferring'
                  ? t('settings.recording.firmwareOtaTransferProgress', '正在发送固件...')
                  : state.userState === 'rebooting'
                    ? t('settings.recording.firmwareOtaFinalizeProgress', '固件已发送，正在校验并准备重启...')
                    : t('settings.recording.firmwareOtaVerifyProgress', '正在重新连接并确认固件版本...')}
              </div>
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

type WiredFlashStatus = 'idle' | 'checking' | 'ready' | 'flashing' | 'repairing' | 'ok' | 'err';

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
  const stm32WbSwd = isStm32WbSwdTarget(selectedPayload?.target);
  const activeWiredPort = stm32WbSwd ? 'SWD' : port;
  const activeWiredBaud = stm32WbSwd ? null : parseBaud(baud) ?? 460800;
  const activePreserveOtaData = stm32WbSwd ? false : preserveOtaData;
  const busy = status === 'checking' || status === 'flashing' || status === 'repairing';
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
        if (isStm32WbSwdTarget(payload.target)) {
          setPort('SWD');
          setBaud('');
          setPreserveOtaData(false);
        }
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

  const startBootRepair = async () => {
    if (!selection) return;
    setStatus('repairing');
    setMessage(null);
    setResult(null);
    try {
      const nextResult = await runWiredOperationWithProgress('bootloaderRepair', () =>
        repairWiredFirmwareBootloader({
          path: selection.path,
          port: activeWiredPort,
          baud: null,
        }),
      );
      setResult(nextResult);
      setProgress(makeDoneWiredProgress('bootloaderRepair', nextResult.port, nextResult.version));
      setStatus('ok');
    } catch (error) {
      setStatus('err');
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

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
        {!stm32WbSwd && (
          <div className="ol-firmware-wired-actions" style={{ display: 'flex', gap: 8, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
            <Btn variant="soft" size="sm" icon="refresh" onClick={() => void refreshPorts()} disabled={busy || disabled}>
              {t('settings.recording.wiredFirmwareRefreshPorts', '串口')}
            </Btn>
          </div>
        )}
      </div>

      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.55 }}>
        {stm32WbSwd
          ? t(
              'settings.recording.wiredFirmwareStm32Desc',
              '使用上方已选择的同一个 Companion 发布包，通过 ST-LINK/SWD 和 STM32CubeProgrammer 写入 OTA loader 与 app。',
            )
          : t(
              'settings.recording.wiredFirmwareDesc',
              '使用上方已选择的同一个固件发布包，通过 USB/串口读取其中的 factory 子包，写入 bootloader、分区表和 app；也可以单独执行 Boot 修复。',
            )}
      </div>

      {!packagePath && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
          {t('settings.recording.wiredFirmwareNeedsSharedPackage', '先在上方选择固件发布包，再选择有线刷机。')}
        </div>
      )}

      {stm32WbSwd ? (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(140px, 1fr))', gap: 8 }}>
          <FirmwareOtaFact label={t('settings.recording.wiredFirmwareInterface', '接口')} value="ST-LINK / SWD" />
          <FirmwareOtaFact label={t('settings.recording.wiredFirmwareProgrammer', '刷机工具')} value="STM32CubeProgrammer" />
        </div>
      ) : (
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
      )}

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

      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
        <Btn
          variant="ghost"
          size="sm"
          icon="bolt"
          onClick={() => void startBootRepair()}
          disabled={!selection || busy || disabled || stm32WbSwd || !selectedPayload?.supportsBootRepair}
        >
          {status === 'repairing' ? t('settings.recording.wiredFirmwareRepairing', '修复中') : t('settings.recording.wiredFirmwareBootRepair', 'Boot 修复')}
        </Btn>
        {stm32WbSwd && selectedPayload && (
          <span style={{ fontSize: 11, color: 'var(--ol-ink-4)' }}>
            {t('settings.recording.wiredFirmwareStm32NoBootRepair', 'STM32WB 通过 ST-LINK/SWD 恢复，不提供 Boot 修复按钮。')}
          </span>
        )}
        {!stm32WbSwd && selectedPayload && !selectedPayload.supportsBootRepair && (
          <span style={{ fontSize: 11, color: 'var(--ol-ink-4)' }}>
            {t('settings.recording.wiredFirmwareBootRepairNeedsFactory', 'Boot 修复需要 factory 包。')}
          </span>
        )}
      </div>

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

function parseBaud(value: string): number | null {
  const parsed = Number.parseInt(value.trim(), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function isStm32WbSwdTarget(target: string | null | undefined): boolean {
  const normalized = (target ?? '').trim().toLowerCase().replace(/[-_]/g, '');
  return normalized === 'nucleowb55rg'
    || normalized === 'stm32wb55rg'
    || normalized === 'companionpendantce'
    || normalized === 'stm32wb55ceux';
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
    case 'repairing':
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
    case 'repairing':
      return t('settings.recording.wiredFirmwareRepairing', '修复中');
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
    return localizeOtaText(language, '升级包版本不高于当前设备固件。', 'The OTA package is not newer than the device firmware.');
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
