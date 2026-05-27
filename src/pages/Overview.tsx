// Overview.tsx — 真实指标，从 listHistory + getCredentials 派生。

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Icon } from '../components/Icon';
import { detectOS } from '../components/WindowChrome';
import { consumePendingDemoMode, OPEN_DEMO_MODE_EVENT } from '../lib/demoMode';
import { summarizeListenerDeviceHealth } from '../lib/deviceHealth';
import {
  embeddedBleProbeErrorMessage,
  runEmbeddedBleProbeWithTimeout,
  type EmbeddedBleProbeStatus,
} from '../lib/embeddedBleProbe';
import { formatComboLabel } from '../lib/hotkey';
import {
  exportDiagnosticPackage,
  getCredentials,
  getEmbeddedBleRuntimeStatus,
  listHistory,
  openSystemSettings,
  setActiveAsrProvider,
  startDictation,
} from '../lib/ipc';
import type { CredentialsStatus, DictationSession, EmbeddedBleRuntimeStatus, PolishMode } from '../lib/types';
import { useHotkeySettings } from '../state/HotkeySettingsContext';
import { Btn, Card, PageHeader, Pill } from './_atoms';
import { EmbeddedBleStatusPanel } from '../components/EmbeddedBleStatusPanel';

function useModeLabels(): Record<PolishMode, string> {
  const { t } = useTranslation();
  return {
    raw: t('style.modes.raw.name'),
    light: t('style.modes.light.name'),
    structured: t('style.modes.structured.name'),
    formal: t('style.modes.formal.name'),
  };
}

interface OverviewProps {
  onOpenHistory?: () => void;
  onOpenProvidersSettings?: () => void;
  onOpenRecordingSettings?: () => void;
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

const DEMO_PLAY_IDS = ['anxiety', 'role', 'polish'] as const;
type DemoPlayId = typeof DEMO_PLAY_IDS[number];

const DEMO_PLAY_ICONS: Record<DemoPlayId, string> = {
  anxiety: 'sparkle',
  role: 'user',
  polish: 'doc',
};

export function Overview({ onOpenHistory, onOpenProvidersSettings, onOpenRecordingSettings }: OverviewProps) {
  const { t } = useTranslation();
  const modeLabel = useModeLabels();
  const [history, setHistory] = useState<DictationSession[]>([]);
  const [historyError, setHistoryError] = useState(false);
  const [credsError, setCredsError] = useState(false);
  const [demoOpen, setDemoOpen] = useState(false);
  const [demoVariant, setDemoVariant] = useState(0);
  const [bleRuntimeStatus, setBleRuntimeStatus] = useState<EmbeddedBleRuntimeStatus | null>(null);
  const [embeddedBleProbeStatus, setEmbeddedBleProbeStatus] = useState<EmbeddedBleProbeStatus>('idle');
  const [embeddedBleProbeMessage, setEmbeddedBleProbeMessage] = useState('');
  const embeddedBleProbeRunId = useRef(0);
  const [creds, setCreds] = useState<CredentialsStatus>({
    activeAsrProvider: 'volcengine',
    activeLlmProvider: 'ark',
    asrConfigured: false,
    llmConfigured: false,
    volcengineConfigured: false,
    arkConfigured: false,
  });
  const { prefs } = useHotkeySettings();

  const refreshHistory = useCallback(() => {
    setHistoryError(false);
    listHistory()
      .then(setHistory)
      .catch(error => {
        console.error('[overview] failed to load history', error);
        setHistoryError(true);
      });
  }, []);

  const refreshBleRuntimeStatus = useCallback(() => {
    getEmbeddedBleRuntimeStatus()
      .then(setBleRuntimeStatus)
      .catch(error => {
        console.warn('[overview] failed to load embedded BLE runtime status', error);
        setBleRuntimeStatus(null);
      });
  }, []);

  useEffect(() => {
    refreshHistory();
    refreshBleRuntimeStatus();
    getCredentials()
      .then(status => {
        setCreds(status);
        setCredsError(false);
      })
      .catch(error => {
        console.error('[overview] failed to load credentials status', error);
        setCredsError(true);
      });
  }, [refreshBleRuntimeStatus, refreshHistory]);

  useEffect(() => {
    const openDemo = () => {
      consumePendingDemoMode();
      setDemoOpen(true);
      setDemoVariant(value => value + 1);
    };
    window.addEventListener(OPEN_DEMO_MODE_EVENT, openDemo);
    if (consumePendingDemoMode()) {
      openDemo();
    }
    return () => window.removeEventListener(OPEN_DEMO_MODE_EVENT, openDemo);
  }, []);

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

  // 周历:过去 7 天每天的条数
  const weekly = useMemo(() => {
    const buckets = Array(7).fill(0);
    const today = new Date();
    today.setHours(0, 0, 0, 0);
    history.forEach(s => {
      const d = new Date(s.createdAt);
      const diff = Math.floor((today.getTime() - d.setHours(0, 0, 0, 0)) / 86400000);
      if (diff >= 0 && diff < 7) {
        buckets[6 - diff] += 1;
      }
    });
    return buckets;
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
      historyError,
    }),
    [
      bleRuntimeStatus?.backgroundListenerActive,
      bleRuntimeStatus?.backgroundListenerDisabledByEnv,
      bleRuntimeStatus?.backgroundListenerLastError,
      bleRuntimeStatus?.backgroundListenerReady,
      bleRuntimeStatus?.wakeRecovery?.status,
      history,
      historyError,
      prefs?.dictationInputSource,
    ],
  );
  const embeddedBleSupported = detectOS() === 'win';
  const deviceHealthDetail = deviceHealth.state === 'healthy'
    ? t('settings.recording.embeddedBleConnectionReady')
    : t(`overview.deviceHealth.reason.${deviceHealth.reason}`);
  const overviewBleStatus: EmbeddedBleProbeStatus = embeddedBleProbeStatus === 'checking' || embeddedBleProbeStatus === 'error'
    ? embeddedBleProbeStatus
    : deviceHealth.state === 'healthy'
      ? 'ok'
      : deviceHealth.state === 'degraded' || deviceHealth.state === 'error'
        ? 'error'
        : embeddedBleProbeStatus === 'ok'
          ? 'ok'
          : 'idle';
  const overviewBleMessage = embeddedBleProbeMessage
    || bleRuntimeStatus?.wakeRecovery?.userGuidance
    || deviceHealthDetail;
  const openBluetoothSettings = useCallback(() => {
    void openSystemSettings('bluetooth').catch(err => {
      console.warn('[overview] open bluetooth settings failed', err);
    });
  }, []);
  const runEmbeddedBleProbe = useCallback(async () => {
    if (!embeddedBleSupported || embeddedBleProbeStatus === 'checking') return;
    const runId = embeddedBleProbeRunId.current + 1;
    embeddedBleProbeRunId.current = runId;
    setEmbeddedBleProbeStatus('checking');
    setEmbeddedBleProbeMessage(t('settings.recording.embeddedBleConnectionMessageChecking'));
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
  const exportBleDiagnostics = useCallback(() => {
    const ts = new Date().toISOString().replace(/[:.]/g, '-');
    void exportDiagnosticPackage(`listener-type-ble-wake-diagnostics-${ts}.json`).catch(err => {
      console.warn('[overview] export diagnostic package failed', err);
    });
  }, []);

  return (
    <>
      <PageHeader title={t('overview.title')} />

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(190px, 1fr))', gap: 12, marginBottom: 12 }}>
        <ProviderCard
          kind={t('overview.asrKind')}
          name={asrProviderName}
          subname={asrProviderId}
          status={credsError ? 'error' : creds.asrConfigured ? 'configured' : 'notConfigured'}
        />
        <ProviderCard
          kind={t('overview.llmKind')}
          name={llmProviderName}
          subname={llmProviderId}
          status={credsError ? 'error' : creds.llmConfigured ? 'configured' : 'notConfigured'}
        />
      </div>

      <div style={{ marginBottom: 18 }}>
        <EmbeddedBleStatusPanel
          supported={embeddedBleSupported}
          status={overviewBleStatus}
          message={overviewBleMessage}
          onOpenBluetoothSettings={openBluetoothSettings}
          onProbe={() => void runEmbeddedBleProbe()}
          onOpenRecordingSettings={onOpenRecordingSettings}
          onExportDiagnostics={exportBleDiagnostics}
        />
      </div>

      {/* Quick start guidance when providers aren't configured */}
      {(!creds.asrConfigured || !creds.llmConfigured) && (
        <QuickStartCard
          asrConfigured={creds.asrConfigured}
          llmConfigured={creds.llmConfigured}
          onConfigure={onOpenProvidersSettings}
          onTryDemo={() => {
            setDemoOpen(true);
            setDemoVariant(value => value + 1);
          }}
          onUseLocal={async () => {
            try {
              await setActiveAsrProvider('foundry-local-whisper');
              const status = await getCredentials();
              setCreds(status);
            } catch { /* ignore */ }
          }}
          onTestRecord={() => { startDictation().catch(() => {}); }}
        />
      )}

      {demoOpen && (
        <DemoModeCard
          variant={demoVariant}
          onRegenerate={() => setDemoVariant(value => value + 1)}
          onSelect={setDemoVariant}
          onClose={() => setDemoOpen(false)}
        />
      )}

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 12, marginBottom: 18 }}>
        <Metric icon="hash" label={t('overview.metricChars')} value={historyError ? '—' : metrics.charsToday.toLocaleString()} trend={historyError ? t('overview.historyLoadError') : t('overview.metricSegments', { count: metrics.segmentsToday })} />
        <Metric icon="mic" label={t('overview.metricDuration')} value={historyError ? '—' : formatDuration(metrics.totalDurationMs, t)} trend={historyError ? t('overview.historyLoadError') : ''} />
        <Metric icon="clock" label={t('overview.metricAvg')} value={historyError ? '—' : formatDuration(metrics.avgLatencyMs, t)} trend={historyError ? t('overview.historyLoadError') : metrics.segmentsToday > 0 ? t('overview.metricAvgTrend') : t('overview.metricNoData')} />
        <Metric icon="bolt" label={t('overview.metricTotal')} value={historyError ? '—' : String(history.length)} trend={historyError ? t('overview.historyLoadError') : t('overview.metricTotalTrend')} accent />
      </div>

      {/* 底部一行 = flex:1 撑满剩余高度（父 wrapper 是 display:flex/column）。
          只有「最近识别」内部允许滚动；其他卡片按内容自然高度，不破裂底部圆角。
          issue #243 follow-up：去掉外层 overflow 后底部圆角被裁的视觉问题。 */}
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1.4fr', gap: 12, flex: 1, minHeight: 0 }}>
        <Card padding={18} style={{ display: 'flex', flexDirection: 'column', minHeight: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 14 }}>
            <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink-2)' }}>{t('overview.weekTitle')}</span>
            <span style={{ fontSize: 11, color: 'var(--ol-ink-4)' }}>{t('overview.weekUnit')}</span>
          </div>
          {historyError ? (
            <div style={{ height: 100, display: 'flex', alignItems: 'center', justifyContent: 'center', textAlign: 'center', fontSize: 12, color: 'var(--ol-ink-4)' }}>
              {t('overview.historyLoadError')}
            </div>
          ) : (
            <WeekChart data={weekly} />
          )}
          <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 10, color: 'var(--ol-ink-4)', marginTop: 8 }}>
            {weekDayLabels(t('overview.weekDays', { returnObjects: true }) as string[]).map((d, i) => <span key={i}>{d}</span>)}
          </div>
        </Card>

        <Card padding={0} style={{ display: 'flex', flexDirection: 'column', minHeight: 0, overflow: 'hidden' }}>
          <div style={{ padding: '14px 18px', borderBottom: '0.5px solid var(--ol-line)', display: 'flex', alignItems: 'center', justifyContent: 'space-between', flexShrink: 0 }}>
            <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--ol-ink-2)' }}>{t('overview.recentTitle')}</span>
            <Btn size="sm" variant="ghost" onClick={onOpenHistory}>{t('overview.recentAll')}</Btn>
          </div>
          <div className="ol-thinscroll" style={{ flex: 1, minHeight: 0, overflow: 'auto' }}>
            {historyError ? (
              <div style={{ padding: 24, textAlign: 'center', fontSize: 12, color: 'var(--ol-ink-4)', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 10 }}>
                <span>{t('overview.recentLoadFailed')}</span>
                <Btn size="sm" variant="ghost" onClick={refreshHistory}>{t('overview.historyRetry')}</Btn>
              </div>
            ) : (
              <>
                {history.length === 0 && (
                  <div style={{ padding: 24, textAlign: 'center', fontSize: 12, color: 'var(--ol-ink-4)' }}>
                    {t('overview.recentEmpty', { trigger: prefs ? formatComboLabel(prefs.dictationHotkey) : '' })}
                  </div>
                )}
                {history.slice(0, 5).map(s => (
                  <RecentRow key={s.id} session={s} modeLabel={modeLabel} />
                ))}
              </>
            )}
          </div>
        </Card>
      </div>
    </>
  );
}

interface ProviderCardProps {
  kind: string;
  name: string;
  subname: string;
  status: 'configured' | 'notConfigured' | 'error';
}

function ProviderCard({ kind, name, subname, status }: ProviderCardProps) {
  const { t } = useTranslation();
  // ASR 卡用 mic 图标，其他用 sparkle —— 通过比较译文判断会随语言改变，故改用本地化无关的字面量比较。
  const isAsr = kind === t('overview.asrKind');
  return (
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

function WeekChart({ data }: { data: number[] }) {
  const max = Math.max(...data, 1);
  return (
    <div style={{ display: 'flex', alignItems: 'flex-end', gap: 8, height: 100 }}>
      {data.map((v, i) => {
        const isToday = i === 6;
        return (
          <div key={i} style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 4 }}>
            <div style={{ fontSize: 9.5, color: isToday ? 'var(--ol-blue)' : 'var(--ol-ink-4)', fontWeight: isToday ? 600 : 400 }}>{v}</div>
            <div
              style={{
                width: '100%',
                height: `${(v / max) * 80}px`,
                minHeight: 2,
                borderRadius: 4,
                background: isToday ? 'var(--ol-blue)' : 'var(--ol-ink)',
                opacity: v === 0 ? 0.15 : isToday ? 1 : 0.85,
                transition: 'height 0.18s var(--ol-motion-soft), opacity 0.18s var(--ol-motion-soft)',
              }}
            />
          </div>
        );
      })}
    </div>
  );
}

function RecentRow({ session, modeLabel }: { session: DictationSession; modeLabel: Record<PolishMode, string> }) {
  const { t } = useTranslation();
  return (
    <div style={{ padding: '12px 18px', borderBottom: '0.5px solid var(--ol-line-soft)', display: 'flex', gap: 12, alignItems: 'flex-start' }}>
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 4, minWidth: 60 }}>
        <span style={{ fontSize: 11, fontFamily: 'var(--ol-font-mono)', color: 'var(--ol-ink-3)' }}>
          {formatTime(session.createdAt)}
        </span>
        <Pill size="sm" tone="default">{modeLabel[session.mode]}</Pill>
      </div>
      <div style={{ flex: 1, fontSize: 12.5, color: 'var(--ol-ink-2)', whiteSpace: 'pre-line', lineHeight: 1.55, overflow: 'hidden', textOverflow: 'ellipsis', display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical' }}>
        {session.finalText.split('\n')[0]}
      </div>
      <span style={{ fontSize: 10.5, color: 'var(--ol-ink-4)', fontFamily: 'var(--ol-font-mono)' }}>
        {formatDuration(session.durationMs ?? 0, t)}
      </span>
    </div>
  );
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  const pad = (n: number) => String(n).padStart(2, '0');
  if (sameDay) return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

function formatDuration(ms: number, t: ReturnType<typeof useTranslation>['t']): string {
  if (ms <= 0) return '—';
  const sec = ms / 1000;
  if (sec < 60) return t('common.durationSeconds', { value: sec.toFixed(1) });
  return `${Math.floor(sec / 60)}:${String(Math.floor(sec % 60)).padStart(2, '0')}`;
}

function weekDayLabels(names: string[]): string[] {
  const today = new Date().getDay();
  const out: string[] = [];
  for (let i = 6; i >= 0; i--) {
    out.push(names[(today - i + 7) % 7]);
  }
  return out;
}

interface QuickStartCardProps {
  asrConfigured: boolean;
  llmConfigured: boolean;
  onConfigure?: () => void;
  onTryDemo?: () => void;
  onUseLocal?: () => void;
  onTestRecord?: () => void;
}

function QuickStartCard({ asrConfigured, llmConfigured, onConfigure, onTryDemo, onUseLocal, onTestRecord }: QuickStartCardProps) {
  const { t } = useTranslation();
  const [dismissed, setDismissed] = useState(false);
  if (dismissed) return null;
  return (
    <Card padding={0} style={{ marginBottom: 18, overflow: 'hidden' }}>
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
          {t('overview.quickStartDesc')}
          {!asrConfigured && !llmConfigured ? '' : !asrConfigured ? ` ASR ${t('overview.statusNotConfigured')}。` : ` LLM ${t('overview.statusNotConfigured')}。`}
        </div>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          {onConfigure && (
            <Btn size="sm" variant="blue" icon="settings" onClick={onConfigure}>{t('overview.quickStartConfigure')}</Btn>
          )}
          {onTryDemo && (
            <Btn size="sm" variant="soft" icon="sparkle" onClick={onTryDemo}>{t('overview.quickStartDemo')}</Btn>
          )}
          {!asrConfigured && onUseLocal && (
            <Btn size="sm" variant="ghost" icon="bolt" onClick={onUseLocal}>{t('overview.quickStartLocal')}</Btn>
          )}
          {onTestRecord && (
            <Btn size="sm" variant="ghost" icon="mic" onClick={onTestRecord}>{t('overview.quickStartTest')}</Btn>
          )}
        </div>
      </div>
    </Card>
  );
}

function DemoModeCard({
  variant,
  onRegenerate,
  onSelect,
  onClose,
}: {
  variant: number;
  onRegenerate: () => void;
  onSelect: (index: number) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const sampleIndex = ((variant % DEMO_PLAY_IDS.length) + DEMO_PLAY_IDS.length) % DEMO_PLAY_IDS.length;
  const playId = DEMO_PLAY_IDS[sampleIndex];
  const baseKey = `overview.demoPlays.${playId}`;

  return (
    <Card padding={0} style={{ marginBottom: 18, overflow: 'hidden', borderColor: 'rgba(101,123,112,0.28)' }}>
      <div style={{ padding: '14px 18px', display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 14, borderBottom: '0.5px solid var(--ol-line)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0 }}>
          <div style={{ width: 30, height: 30, borderRadius: 8, background: 'var(--ol-blue-soft)', color: 'var(--ol-blue)', display: 'flex', alignItems: 'center', justifyContent: 'center', flexShrink: 0 }}>
            <Icon name="sparkle" size={15} />
          </div>
          <div style={{ minWidth: 0 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
              <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--ol-ink)' }}>{t('overview.demoTitle')}</span>
              <Pill tone="blue" size="sm">{t('overview.demoBadge')}</Pill>
            </div>
            <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginTop: 2 }}>
              {t('overview.demoDesc')}
            </div>
          </div>
        </div>
        <Btn size="sm" variant="ghost" onClick={onClose}>{t('common.close')}</Btn>
      </div>
      <div style={{ padding: '12px 18px 0', display: 'flex', gap: 8, flexWrap: 'wrap' }}>
        {DEMO_PLAY_IDS.map((id, index) => {
          const selected = id === playId;
          return (
            <Btn
              key={id}
              size="sm"
              variant={selected ? 'blue' : 'ghost'}
              icon={DEMO_PLAY_ICONS[id]}
              onClick={() => onSelect(index)}
              style={{ borderColor: selected ? 'transparent' : 'var(--ol-line)' }}
            >
              {t(`overview.demoPlays.${id}.title`)}
            </Btn>
          );
        })}
      </div>
      <div style={{ padding: '14px 18px 12px', display: 'grid', gridTemplateColumns: 'minmax(0, 0.9fr) minmax(0, 1.1fr)', gap: 12 }}>
        <DemoPane label={t('overview.demoInputLabel')} text={t(`${baseKey}.input`)} />
        <DemoPane label={t('overview.demoOutputLabel')} text={t(`${baseKey}.output`)} accent />
      </div>
      <div style={{ padding: '0 18px 14px', display: 'grid', gridTemplateColumns: 'minmax(0, 0.9fr) minmax(0, 1.1fr)', gap: 12 }}>
        <DemoPane label={t('overview.demoStyleLabel')} text={t(`${baseKey}.style`)} compact />
        <DemoPane label={t('overview.demoFallbackLabel')} text={t(`${baseKey}.fallback`)} compact />
      </div>
      <div style={{ padding: '0 18px 16px', display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12, flexWrap: 'wrap' }}>
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
          {t('overview.demoConfigNote')}
        </div>
        <Btn size="sm" variant="blue" icon="refresh" onClick={onRegenerate}>{t('overview.demoRegenerate')}</Btn>
      </div>
    </Card>
  );
}

function DemoPane({ label, text, accent = false, compact = false }: { label: string; text: string; accent?: boolean; compact?: boolean }) {
  return (
    <div
      style={{
        minWidth: 0,
        padding: compact ? '10px 12px' : '12px 14px',
        borderRadius: 8,
        background: accent ? 'var(--ol-blue-soft)' : 'var(--ol-surface-2)',
        border: accent ? '0.5px solid rgba(101,123,112,0.20)' : '0.5px solid var(--ol-line-soft)',
      }}
    >
      <div style={{ fontSize: 10.5, color: accent ? 'var(--ol-blue)' : 'var(--ol-ink-4)', fontWeight: 600, letterSpacing: 0, textTransform: 'uppercase', marginBottom: 7 }}>
        {label}
      </div>
      <div style={{ fontSize: compact ? 11.5 : 12.5, color: 'var(--ol-ink-2)', lineHeight: compact ? 1.5 : 1.6, whiteSpace: 'pre-line' }}>
        {text}
      </div>
    </div>
  );
}
