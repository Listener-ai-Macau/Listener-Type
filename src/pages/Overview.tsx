// Overview.tsx — 真实指标，从 listHistory + getCredentials 派生。

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Icon } from '../components/Icon';
import { detectOS } from '../components/WindowChrome';
import { summarizeListenerDeviceHealth } from '../lib/deviceHealth';
import { buildBleRecoveryUi } from '../lib/bleRecoveryUi';
import {
  embeddedBleProbeErrorMessage,
  runEmbeddedBleProbeWithTimeout,
  type EmbeddedBleProbeStatus,
} from '../lib/embeddedBleProbe';
import { formatComboLabel } from '../lib/hotkey';
import { CREDENTIALS_STATUS_CHANGED_EVENT } from '../lib/credentialsStatusEvents';
import { subscribeHistoryUpdates } from '../lib/historyUpdates';
import {
  exportDiagnosticPackage,
  getCredentials,
  getEmbeddedBleRuntimeStatus,
  listHistory,
  openSystemSettings,
  recoverEmbeddedBleDevice,
  startDictation,
} from '../lib/ipc';
import type { CredentialsStatus, DictationSession, EmbeddedBleRepairResult, EmbeddedBleRuntimeStatus, PolishMode } from '../lib/types';
import { useHotkeySettings } from '../state/HotkeySettingsContext';
import { Btn, Card, PageHeader, Pill } from './_atoms';
import { EmbeddedBleStatusPanel } from '../components/EmbeddedBleStatusPanel';
import { DeviceLayoutsDialog } from '../components/DeviceLayoutsDialog';
import { writingDeviceKeyProfile } from '../lib/deviceKeyLayouts';
import { hardwarePhoto } from '../components/DeviceLayoutMap';

interface OverviewProps {
  onOpenProvidersSettings?: () => void;
  onOpenRecordingSettings?: () => void;
  onOpenLocalAsrSettings?: () => void;
  onStartDemoMode?: () => void;
  onOpenStyles?: () => void;
  onOpenAudioFile?: () => void;
}

const ASR_NAME_KEY_BY_ID: Record<string, string> = {
  volcengine: 'asrVolcengine',
  bailian: 'asrBailian',
  siliconflow: 'asrSiliconflow',
  zhipu: 'asrZhipu',
  groq: 'asrGroq',
  whisper: 'asrWhisper',
  'foundry-local-whisper': 'asrFoundryLocalWhisper',
  'local-qwen3': 'asrLocalQwen3',
};

const LLM_NAME_KEY_BY_ID: Record<string, string> = {
  ark: 'ark',
  deepseek: 'deepseek',
  siliconflow: 'siliconflow',
  openai: 'openai',
  codex_oauth: 'codexOAuth',
  mimo: 'mimo',
  cometapi: 'cometapi',
  openrouterFree: 'openrouterFree',
  alibabaCoding: 'alibabaCoding',
  codingPlanX: 'codingPlanX',
  custom: 'custom',
};

export function Overview({ onOpenProvidersSettings, onOpenRecordingSettings, onOpenLocalAsrSettings, onStartDemoMode, onOpenStyles, onOpenAudioFile }: OverviewProps) {
  const { t } = useTranslation();
  const [layoutsOpen, setLayoutsOpen] = useState<'configure' | 'practice' | 'phrases' | null>(null);
  const [history, setHistory] = useState<DictationSession[]>([]);
  const [historyError, setHistoryError] = useState(false);
  const [credsError, setCredsError] = useState(false);
  const [bleRuntimeStatus, setBleRuntimeStatus] = useState<EmbeddedBleRuntimeStatus | null>(null);
  const [embeddedBleProbeStatus, setEmbeddedBleProbeStatus] = useState<EmbeddedBleProbeStatus>('idle');
  const [embeddedBleProbeMessage, setEmbeddedBleProbeMessage] = useState('');
  const [lastBleRepairResult, setLastBleRepairResult] = useState<EmbeddedBleRepairResult | null>(null);
  const [bleDiagnosticStatus, setBleDiagnosticStatus] = useState<'idle' | 'busy' | 'ok' | 'err'>('idle');
  const embeddedBleProbeRunId = useRef(0);
  const [creds, setCreds] = useState<CredentialsStatus>({
    activeAsrProvider: 'volcengine',
    activeLlmProvider: 'ark',
    asrConfigured: false,
    llmConfigured: false,
    volcengineConfigured: false,
    arkConfigured: false,
  });
  const { prefs, updatePrefs } = useHotkeySettings();
  const historyMounted = useRef(true);
  const historyRead = useRef(0);

  const refreshHistory = useCallback(() => {
    const request = ++historyRead.current;
    setHistoryError(false);
    listHistory()
      .then(value => {
        if (historyMounted.current && request === historyRead.current) setHistory(value);
      })
      .catch(error => {
        if (!historyMounted.current || request !== historyRead.current) return;
        console.error('[overview] failed to load history', error);
        setHistoryError(true);
      });
  }, []);

  useEffect(() => {
    historyMounted.current = true;
    const unsubscribe = subscribeHistoryUpdates(refreshHistory);
    return () => {
      historyMounted.current = false;
      historyRead.current += 1;
      unsubscribe();
    };
  }, [refreshHistory]);

  const refreshBleRuntimeStatus = useCallback(() => {
    getEmbeddedBleRuntimeStatus()
      .then(setBleRuntimeStatus)
      .catch(error => {
        console.warn('[overview] failed to load embedded BLE runtime status', error);
        setBleRuntimeStatus(null);
      });
  }, []);

  useEffect(() => {
    let disposed = false;
    let latestRead = 0;
    const refreshCredentials = () => {
      const request = ++latestRead;
      void getCredentials()
        .then(status => {
          if (disposed || request !== latestRead) return;
          setCreds(status);
          setCredsError(false);
        })
        .catch(error => {
          if (disposed || request !== latestRead) return;
          console.error('[overview] failed to load credentials status', error);
          setCredsError(true);
        });
    };
    window.addEventListener(CREDENTIALS_STATUS_CHANGED_EVENT, refreshCredentials);
    refreshHistory();
    refreshBleRuntimeStatus();
    refreshCredentials();
    return () => {
      disposed = true;
      window.removeEventListener(CREDENTIALS_STATUS_CHANGED_EVENT, refreshCredentials);
    };
  }, [refreshBleRuntimeStatus, refreshHistory]);

  const metrics = useMemo(() => {
    const today = new Date();
    today.setHours(0, 0, 0, 0);
    const todays = history.filter(s => new Date(s.createdAt) >= today);
    const charsToday = todays.reduce((acc, s) => acc + s.finalText.length, 0);
    const segmentsToday = todays.length;
    const totalDurationMs = todays.reduce((acc, s) => acc + (s.durationMs ?? 0), 0);
    const avgLatencyMs = segmentsToday > 0 ? totalDurationMs / segmentsToday : 0;
    return { charsToday, segmentsToday, totalDurationMs, avgLatencyMs };
  }, [history]);

  const asrProviderId = creds.activeAsrProvider || 'volcengine';
  const llmProviderId = creds.activeLlmProvider || 'ark';
  const asrNameKey = ASR_NAME_KEY_BY_ID[asrProviderId];
  const llmNameKey = LLM_NAME_KEY_BY_ID[llmProviderId];
  const asrProviderName = asrNameKey
    ? t(`settings.providers.presets.${asrNameKey}`)
    : asrProviderId;
  const llmProviderName = llmNameKey
    ? t(`settings.providers.presets.${llmNameKey}`)
    : llmProviderId;
  const deviceHealth = useMemo(
    () => summarizeListenerDeviceHealth({
      dictationInputSource: prefs?.dictationInputSource,
      os: detectOS(),
      history,
      backgroundListenerDisabled: bleRuntimeStatus?.backgroundListenerDisabledByEnv ?? false,
      backgroundListenerActive: bleRuntimeStatus?.backgroundListenerActive ?? false,
      backgroundListenerReady: bleRuntimeStatus?.backgroundListenerReady ?? false,
      backgroundListenerError: bleRuntimeStatus?.backgroundListenerLastError ?? null,
      wakeRecoveryStatus: bleRuntimeStatus?.wakeRecovery?.status ?? null,
      usbPowered: bleRuntimeStatus?.wakeRecovery?.usbPowered ?? null,
      historyError,
    }),
    [
      bleRuntimeStatus?.backgroundListenerActive,
      bleRuntimeStatus?.backgroundListenerDisabledByEnv,
      bleRuntimeStatus?.backgroundListenerLastError,
      bleRuntimeStatus?.backgroundListenerReady,
      bleRuntimeStatus?.wakeRecovery?.status,
      bleRuntimeStatus?.wakeRecovery?.usbPowered,
      history,
      historyError,
      prefs?.dictationInputSource,
    ],
  );
  const embeddedBleSupported = detectOS() === 'win';
  useEffect(() => {
    if (!embeddedBleSupported || (prefs?.dictationInputSource ?? 'embeddedBle') !== 'embeddedBle') {
      return;
    }
    const interval = window.setInterval(refreshBleRuntimeStatus, 3000);
    return () => window.clearInterval(interval);
  }, [embeddedBleSupported, prefs?.dictationInputSource, refreshBleRuntimeStatus]);

  const bleRecoveryUi = useMemo(
    () => buildBleRecoveryUi({
      supported: embeddedBleSupported,
      probeStatus: embeddedBleProbeStatus,
      probeMessage: embeddedBleProbeMessage,
      runtime: bleRuntimeStatus,
      deviceHealth,
      lastRepairResult: lastBleRepairResult,
    }, t),
    [
      bleRuntimeStatus,
      deviceHealth,
      embeddedBleProbeMessage,
      embeddedBleProbeStatus,
      embeddedBleSupported,
      lastBleRepairResult,
      t,
    ],
  );
  const openBluetoothSettings = useCallback(() => {
    void openSystemSettings('bluetooth').catch(err => {
      console.warn('[overview] open bluetooth settings failed', err);
    });
  }, []);
  const useMicrophoneInput = useCallback(() => {
    void updatePrefs(current => ({
      ...current,
      dictationInputSource: 'microphone',
      dictationInputSourceUserOverridden: true,
    })).catch(err => {
      console.warn('[overview] switch to microphone failed', err);
    });
  }, [updatePrefs]);
  const runEmbeddedBleProbe = useCallback(async () => {
    if (!embeddedBleSupported || embeddedBleProbeStatus === 'checking') return;
    const runId = embeddedBleProbeRunId.current + 1;
    embeddedBleProbeRunId.current = runId;
    setEmbeddedBleProbeStatus('checking');
    setEmbeddedBleProbeMessage(t('settings.recording.embeddedBleConnectionMessageChecking'));
    setLastBleRepairResult(null);
    try {
      await runEmbeddedBleProbeWithTimeout();
      if (embeddedBleProbeRunId.current !== runId) return;
      setEmbeddedBleProbeStatus('ok');
      setEmbeddedBleProbeMessage(t('settings.recording.embeddedBleConnectionReady'));
      refreshHistory();
      refreshBleRuntimeStatus();
    } catch (err) {
      if (embeddedBleProbeRunId.current !== runId) return;
      setEmbeddedBleProbeStatus('error');
      setEmbeddedBleProbeMessage(embeddedBleProbeErrorMessage(err, t));
      refreshBleRuntimeStatus();
    }
  }, [embeddedBleProbeStatus, embeddedBleSupported, refreshBleRuntimeStatus, refreshHistory, t]);
  const recoverEmbeddedBle = useCallback(async () => {
    if (!embeddedBleSupported || embeddedBleProbeStatus === 'checking') return;
    const runId = embeddedBleProbeRunId.current + 1;
    embeddedBleProbeRunId.current = runId;
    setEmbeddedBleProbeStatus('checking');
    setEmbeddedBleProbeMessage(t('settings.recording.embeddedBleConnectionMessageChecking'));
    setLastBleRepairResult(null);
    try {
      const result = await recoverEmbeddedBleDevice(15_000);
      if (embeddedBleProbeRunId.current !== runId) return;
      setLastBleRepairResult(result);
      setBleRuntimeStatus(result.runtime);
      setEmbeddedBleProbeStatus(result.recovered ? 'ok' : 'error');
      setEmbeddedBleProbeMessage(result.message ?? '');
      if (result.openBluetoothSettings) {
        openBluetoothSettings();
      }
      refreshHistory();
      refreshBleRuntimeStatus();
    } catch (err) {
      if (embeddedBleProbeRunId.current !== runId) return;
      setEmbeddedBleProbeStatus('error');
      setLastBleRepairResult(null);
      setEmbeddedBleProbeMessage(embeddedBleProbeErrorMessage(err, t));
      refreshBleRuntimeStatus();
    }
  }, [embeddedBleProbeStatus, embeddedBleSupported, openBluetoothSettings, refreshBleRuntimeStatus, refreshHistory, t]);
  const exportBleDiagnostics = useCallback(async () => {
    const ts = new Date().toISOString().replace(/[:.]/g, '-');
    setBleDiagnosticStatus('busy');
    try {
      const target = await exportDiagnosticPackage(`listener-type-ble-wake-diagnostics-device-${ts}.zip`);
      setBleDiagnosticStatus(target ? 'ok' : 'idle');
    } catch (err) {
      console.warn('[overview] export diagnostic package failed', err);
      setBleDiagnosticStatus('err');
    }
  }, []);

  return (
    <>
      <PageHeader title={t('overview.title')} />

      <Card style={{ marginBottom: 14 }}>
        <div style={{ display: 'flex', gap: 10, alignItems: 'center', marginBottom: 10 }}>
          <Icon name="mic" size={17} />
          <h2 style={{ margin: 0, fontSize: 14, fontWeight: 600 }}>{t('overview.firstDictationTitle')}</h2>
        </div>
        <ol style={{ margin: '0 0 12px', paddingLeft: 20, fontSize: 12.5, lineHeight: 1.8, color: 'var(--ol-ink-2)' }}>
          <li>{t('overview.firstDictationFocus')}</li>
          <li>{t('overview.firstDictationStart', { trigger: prefs ? formatComboLabel(prefs.dictationHotkey) : t('common.loading') })}</li>
          <li>{t('overview.firstDictationFinish', { trigger: prefs ? formatComboLabel(prefs.dictationHotkey) : t('common.loading') })}</li>
        </ol>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
          {!credsError && !creds.asrConfigured && onOpenProvidersSettings && (
            <Btn size="sm" variant="blue" icon="settings" onClick={onOpenProvidersSettings}>{t('overview.firstDictationSetup')}</Btn>
          )}
          {onOpenRecordingSettings && (
            <Btn size="sm" variant="ghost" icon="mic" onClick={onOpenRecordingSettings}>{t('overview.firstDictationInput')}</Btn>
          )}
          <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)' }}>{t('overview.firstDictationCancel')}</span>
        </div>
        {onOpenStyles && (
          <div style={{ borderTop: '1px solid var(--ol-line)', paddingTop: 10, marginTop: 12, display: 'flex', alignItems: 'center', flexWrap: 'wrap', gap: 10 }}>
            <Btn size="sm" variant="ghost" onClick={onOpenStyles}>{t('overview.tryTypedText')}</Btn>
            <span style={{ fontSize: 12, lineHeight: 1.6, color: 'var(--ol-ink-3)' }}>{t('overview.tryTypedTextHint')}</span>
          </div>
        )}
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 10, alignItems: 'center', marginTop: 12 }}>
          {onOpenAudioFile && <Btn size="sm" variant="ghost" onClick={onOpenAudioFile}>{t('audioFile.title')}</Btn>}
          <Btn size="sm" variant="ghost" onClick={() => setLayoutsOpen('practice')}>{t('devicePractice.open')}</Btn>
          <Btn size="sm" variant="ghost" onClick={() => setLayoutsOpen('configure')}>{t('deviceLayouts.open')}</Btn>
          <Btn size="sm" variant="ghost" onClick={() => setLayoutsOpen('phrases')}>{t('savedPhrases.title')}</Btn>
          <span style={{ fontSize: 12, color: 'var(--ol-ink-3)' }}>{t('deviceLayouts.entryHint')}</span>
        </div>
      </Card>
      {layoutsOpen && <DeviceLayoutsDialog rememberSelection initialPractice={layoutsOpen === 'practice'} initialPhrases={layoutsOpen === 'phrases'} initialProfile={layoutsOpen === 'practice' ? writingDeviceKeyProfile() : undefined} onClose={() => setLayoutsOpen(null)} />}

      <Card style={{ marginBottom: 14 }}>
        <div className="ol-listener-hardware-entry">
          <img src={hardwarePhoto} alt={t('hardwareLayout.photo')} width={210} height={148} />
          <div>
            <h2 style={{ margin: '0 0 8px', fontSize: 16 }}>{t('hardwareLayout.entryTitle')}</h2>
            <p className="ol-device-layout-hint">{t('hardwareLayout.entryBody')}</p>
            <Btn size="sm" variant="ghost" onClick={() => setLayoutsOpen('configure')}>{t('hardwareLayout.design')}</Btn>
            <p className="ol-device-layout-hint">{t('hardwareLayout.price')}</p>
          </div>
        </div>
      </Card>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(190px, 1fr))', gap: 12, marginBottom: 12 }}>
        <ProviderCard
          kind={t('overview.asrKind')}
          name={asrProviderName}
          subname={asrProviderId}
          status={credsError ? 'error' : creds.asrConfigured ? 'configured' : 'notConfigured'}
          onOpenSettings={onOpenProvidersSettings}
        />
        <ProviderCard
          kind={t('overview.llmKind')}
          name={llmProviderName}
          subname={llmProviderId}
          status={credsError ? 'error' : creds.llmConfigured ? 'configured' : 'notConfigured'}
          onOpenSettings={onOpenProvidersSettings}
        />
      </div>

      <div style={{ marginBottom: 18 }}>
        <EmbeddedBleStatusPanel
          recovery={bleRecoveryUi}
          onOpenBluetoothSettings={openBluetoothSettings}
          onProbe={() => void runEmbeddedBleProbe()}
          onRepair={() => void recoverEmbeddedBle()}
          onOpenRecordingSettings={onOpenRecordingSettings}
          onUseMicrophone={useMicrophoneInput}
          onExportDiagnostics={() => void exportBleDiagnostics()}
          diagnosticStatus={bleDiagnosticStatus}
        />
      </div>

      {/* Quick start guidance when providers aren't configured */}
      {(!creds.asrConfigured || !creds.llmConfigured) && (
        <QuickStartCard
          asrConfigured={creds.asrConfigured}
          llmConfigured={creds.llmConfigured}
          onConfigure={onOpenProvidersSettings}
          onStartDemo={onStartDemoMode}
          onUseLocal={onOpenLocalAsrSettings}
          onTestRecord={startDictation}
        />
      )}

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 12, marginBottom: 18 }}>
        <Metric icon="hash" label={t('overview.metricChars')} value={historyError ? '—' : metrics.charsToday.toLocaleString()} trend={historyError ? t('overview.historyLoadError') : t('overview.metricSegments', { count: metrics.segmentsToday })} />
        <Metric icon="mic" label={t('overview.metricDuration')} value={historyError ? '—' : formatDuration(metrics.totalDurationMs, t)} trend={historyError ? t('overview.historyLoadError') : ''} />
        <Metric icon="clock" label={t('overview.metricAvg')} value={historyError ? '—' : formatDuration(metrics.avgLatencyMs, t)} trend={historyError ? t('overview.historyLoadError') : metrics.segmentsToday > 0 ? t('overview.metricAvgTrend') : t('overview.metricNoData')} />
        <Metric icon="bolt" label={t('overview.metricTotal')} value={historyError ? '—' : String(history.length)} trend={historyError ? t('overview.historyLoadError') : t('overview.metricTotalTrend')} accent />
      </div>

    </>
  );
}

interface ProviderCardProps {
  kind: string;
  name: string;
  subname: string;
  status: 'configured' | 'notConfigured' | 'error';
  onOpenSettings?: () => void;
}

function ProviderCard({ kind, name, subname, status, onOpenSettings }: ProviderCardProps) {
  const { t } = useTranslation();
  // ASR 卡用 mic 图标，其他用 sparkle —— 通过比较译文判断会随语言改变，故改用本地化无关的字面量比较。
  const isAsr = kind === t('overview.asrKind');
  const interactive = status !== 'configured' && Boolean(onOpenSettings);
  const card = (
    <Card padding={16} style={{ display: 'flex', alignItems: 'center', gap: 14 }}>
      <div
        style={{
          width: 38, height: 38, borderRadius: 10,
          background: 'var(--ol-blue-soft)',
          color: 'var(--ol-blue)',
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }}
      >
        <Icon name={isAsr ? 'mic' : 'sparkle'} size={18} />
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 2 }}>
          <span style={{ fontSize: 11, color: 'var(--ol-ink-4)', fontWeight: 600, letterSpacing: 0, textTransform: 'uppercase' }}>{kind}</span>
          {status === 'configured' && (
            <Pill tone="ok" size="sm">
              <span style={{ width: 5, height: 5, borderRadius: 999, background: 'var(--ol-ok)' }} />
              {t('overview.statusConfigured')}
            </Pill>
          )}
          {status === 'notConfigured' && (
            <Pill tone="outline" size="sm">{t('overview.statusNotConfigured')}</Pill>
          )}
          {status === 'error' && (
            <Pill tone="outline" size="sm" style={{ color: 'var(--ol-red, #ef4444)', borderColor: 'rgba(239,68,68,0.24)' }}>{t('overview.statusUnknown')}</Pill>
          )}
        </div>
        <div style={{ fontSize: 14, fontWeight: 600, color: 'var(--ol-ink)' }}>{name}</div>
        <div style={{ fontSize: 11.5, color: status === 'error' ? 'var(--ol-red, #ef4444)' : 'var(--ol-ink-3)', marginTop: 1, fontFamily: status === 'error' ? undefined : 'var(--ol-font-mono)' }}>
          {status === 'error' ? t('overview.credentialsLoadError') : subname}
        </div>
      </div>
    </Card>
  );
  if (!interactive) return card;
  return (
    <button
      type="button"
      onClick={onOpenSettings}
      style={{
        appearance: 'none',
        WebkitAppearance: 'none',
        border: 0,
        background: 'transparent',
        padding: 0,
        margin: 0,
        width: '100%',
        font: 'inherit',
        textAlign: 'left',
        cursor: 'default',
      }}
    >
      {card}
    </button>
  );
}

interface MetricProps {
  icon: string;
  label: string;
  value: string;
  trend: string;
  accent?: boolean;
}

function Metric({ icon, label, value, trend, accent }: MetricProps) {
  return (
    <Card padding={16}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 8, color: 'var(--ol-ink-3)' }}>
        <Icon name={icon} size={13} />
        <span style={{ fontSize: 11.5 }}>{label}</span>
      </div>
      <div style={{ fontSize: 26, fontWeight: 600, letterSpacing: 0, color: accent ? 'var(--ol-blue)' : 'var(--ol-ink)', lineHeight: 1.1 }}>{value}</div>
      <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', marginTop: 6 }}>{trend || ' '}</div>
    </Card>
  );
}

function formatDuration(ms: number, t: ReturnType<typeof useTranslation>['t']): string {
  if (ms <= 0) return '—';
  const sec = ms / 1000;
  if (sec < 60) return t('common.durationSeconds', { value: sec.toFixed(1) });
  return `${Math.floor(sec / 60)}:${String(Math.floor(sec % 60)).padStart(2, '0')}`;
}

interface QuickStartCardProps {
  asrConfigured: boolean;
  llmConfigured: boolean;
  onConfigure?: () => void;
  onStartDemo?: () => void;
  onUseLocal?: () => void;
  onTestRecord?: () => Promise<void>;
}

function QuickStartCard({ asrConfigured, llmConfigured, onConfigure, onStartDemo, onUseLocal, onTestRecord }: QuickStartCardProps) {
  const { t } = useTranslation();
  const [dismissed, setDismissed] = useState(false);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const startPending = useRef(false);
  const onStart = async () => {
    if (!asrConfigured || !onTestRecord || startPending.current) return;
    startPending.current = true;
    setStarting(true);
    setStartError(null);
    try {
      await onTestRecord();
    } catch (error) {
      setStartError(t('overview.quickStartFailed', { err: error instanceof Error ? error.message : String(error) }));
    } finally {
      startPending.current = false;
      setStarting(false);
    }
  };
  if (dismissed) return null;
  return (
    <Card padding={0} style={{ marginBottom: 18, overflow: 'hidden', flexShrink: 0 }}>
      <div style={{ padding: '14px 18px', display: 'flex', alignItems: 'center', justifyContent: 'space-between', borderBottom: '0.5px solid var(--ol-line)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <div style={{ width: 28, height: 28, borderRadius: 8, background: 'var(--ol-blue-soft)', color: 'var(--ol-blue)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
            <Icon name="sparkle" size={14} />
          </div>
          <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--ol-ink)' }}>{t('overview.quickStartTitle')}</span>
        </div>
        <Btn size="sm" variant="ghost" onClick={() => setDismissed(true)}>{t('overview.quickStartDismiss')}</Btn>
      </div>
      <div style={{ padding: '12px 18px 16px' }}>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-3)', marginBottom: 12, lineHeight: 1.5 }}>
          {t(asrConfigured ? 'overview.quickStartAsrReady' : 'overview.quickStartDesc')}
        </div>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          {onConfigure && (
            <Btn size="sm" variant={asrConfigured ? 'ghost' : 'blue'} icon="settings" onClick={onConfigure}>
              {t(asrConfigured && !llmConfigured ? 'overview.quickStartOptionalPolish' : 'overview.quickStartConfigure')}
            </Btn>
          )}
          {onStartDemo && (
            <Btn size="sm" variant="ghost" icon="sparkle" onClick={onStartDemo}>{t('overview.quickStartDemo')}</Btn>
          )}
          {!asrConfigured && onUseLocal && (
            <Btn size="sm" variant="ghost" icon="bolt" onClick={onUseLocal}>{t('overview.quickStartLocal')}</Btn>
          )}
          {asrConfigured && onTestRecord && (
            <Btn size="sm" variant="ghost" icon="mic" disabled={starting} onClick={() => void onStart()}>{t(starting ? 'overview.quickStartStarting' : 'overview.quickStartTest')}</Btn>
          )}
        </div>
        {startError && <div role="alert" style={{ marginTop: 10, fontSize: 12, color: 'var(--ol-err)', lineHeight: 1.5 }}>{startError}</div>}
      </div>
    </Card>
  );
}
