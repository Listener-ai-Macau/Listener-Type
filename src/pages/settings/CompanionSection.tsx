import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  probeEmbeddedAudioBleSubscription,
} from '../../lib/ipc';
import type { EmbeddedAudioSubmissionResult } from '../../lib/types';
import { Btn, Card, Pill } from '../_atoms';
import { FirmwareOtaPanel } from './FirmwareOtaPanel';
import type { EmbeddedBleProbeStatus } from '../../components/EmbeddedBleStatusPanel';

export function CompanionSection() {
  return (
    <>
      <CompanionBringupCard />
      <FirmwareOtaPanel
        supported
        bleStatus={COMPANION_FIRMWARE_PANEL_BLE_STATUS}
      />
    </>
  );
}

async function submitCompanionEmbeddedAudioBleStream(timeoutMs: number): Promise<EmbeddedAudioSubmissionResult> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<EmbeddedAudioSubmissionResult>('submit_embedded_audio_ble_stream', { timeoutMs });
}

type CompanionAction = 'probe' | 'a1' | 'a2';

const COMPANION_PROBE_TIMEOUT_MS = 30000;
const COMPANION_A1_TIMEOUT_MS = 45000;
const COMPANION_A1_ROUNDS = 6;
const COMPANION_A2_TIMEOUT_MS = 120000;
const COMPANION_FIRMWARE_PANEL_BLE_STATUS: EmbeddedBleProbeStatus = 'idle';

interface CompanionRunResult {
  status: 'idle' | 'running' | 'pass' | 'fail';
  action: CompanionAction | null;
  message: string;
  stats: EmbeddedAudioSubmissionResult['stats'] | null;
}

function CompanionBringupCard() {
  const { t } = useTranslation();
  const [copied, setCopied] = useState('');
  const [run, setRun] = useState<CompanionRunResult>({
    status: 'idle',
    action: null,
    message: '',
    stats: null,
  });
  const hardwareAcceptanceCommand = [
    'pwsh -NoProfile -File <ai-collaboration-workflow>\\scripts\\aiw.ps1',
    'with-lock -Resource Companion-WB55,BLE-Companion',
    '-Purpose "Companion hardware acceptance"',
    '-Run pwsh -NoProfile -File .\\tools\\verify_companion_hardware_acceptance.ps1',
    '-Execute -WorkflowLockHeld',
    '-PackagePath <companion-package.zip>',
    '-EvidencePath <companion_ui_screenshot_or_log>',
  ].join(' ');
  const wiredPackageFlashCommand = [
    'pwsh -NoProfile -File <ai-collaboration-workflow>\\scripts\\aiw.ps1',
    'with-lock -Resource Companion-WB55',
    '-Purpose "Companion wired factory flash validation"',
    '-Run pwsh -NoProfile -File .\\tools\\verify_companion_wired_flash.ps1',
    '-Execute -WorkflowLockHeld',
    '-PackagePath <companion-package.zip>',
  ].join(' ');
  const commands = [
    {
      id: 'readiness',
      label: t('settings.companion.readinessCommand', '总预检'),
      value: 'pwsh -NoProfile -File .\\tools\\verify_companion_bringup_readiness.ps1',
    },
    {
      id: 'build',
      label: t('settings.companion.buildCommand', '编译 bring-up'),
      value: 'pwsh -NoProfile -File .\\tools\\build_nucleo_bringup.ps1',
    },
    {
      id: 'package',
      label: t('settings.companion.packageCommand', '打包同包'),
      value: 'pwsh -NoProfile -File .\\tools\\package_ota_firmware.ps1 -Channel development -BoardTarget NUCLEO-WB55RG',
    },
    {
      id: 'verifyPackage',
      label: t('settings.companion.verifyPackageCommand', '校验同包'),
      value: 'pwsh -NoProfile -File .\\tools\\verify_type_companion_package_static.ps1',
    },
    {
      id: 'hardwareAcceptance',
      label: t('settings.companion.hardwareAcceptanceCommand', '总体验收'),
      value: hardwareAcceptanceCommand,
    },
    {
      id: 'flash',
      label: t('settings.companion.flashCommand', '刷入 NUCLEO'),
      value: 'pwsh -NoProfile -File .\\tools\\flash_nucleo_bringup.ps1',
    },
    {
      id: 'flashPackage',
      label: t('settings.companion.flashPackageCommand', '同包有线'),
      value: wiredPackageFlashCommand,
    },
    {
      id: 'capture',
      label: t('settings.companion.captureCommand', '抓串口日志'),
      value: 'pwsh -NoProfile -File .\\tools\\capture_nucleo_serial.ps1 -Port COM13 -Seconds 30',
    },
  ];
  const busy = run.status === 'running';
  const copy = async (id: string, value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(id);
      window.setTimeout(() => setCopied(current => (current === id ? '' : current)), 1400);
    } catch (error) {
      console.warn('[companion] copy command failed', error);
    }
  };
  const runAction = async (action: CompanionAction, execute: () => Promise<EmbeddedAudioSubmissionResult | void>) => {
    setRun({
      status: 'running',
      action,
      message: companionActionRunningLabel(action, t),
      stats: null,
    });
    try {
      const result = await execute();
      setRun({
        status: 'pass',
        action,
        message: companionActionPassLabel(action, result ?? null, t),
        stats: result?.stats ?? null,
      });
    } catch (error) {
      setRun({
        status: 'fail',
        action,
        message: error instanceof Error ? error.message : String(error),
        stats: null,
      });
    }
  };
  const runCompanionA1Rounds = async () => {
    const results: EmbeddedAudioSubmissionResult[] = [];
    for (let round = 1; round <= COMPANION_A1_ROUNDS; round += 1) {
      setRun({
        status: 'running',
        action: 'a1',
        message: t(
          'settings.companion.a1RoundPending',
          '正在等待 fixture VKA1 短链路会话 {{round}}/{{total}}。',
          { round, total: COMPANION_A1_ROUNDS },
        ),
        stats: results.length ? aggregateCompanionEmbeddedAudioResults(results).stats : null,
      });
      results.push(await submitCompanionEmbeddedAudioBleStream(COMPANION_A1_TIMEOUT_MS));
    }
    return aggregateCompanionEmbeddedAudioResults(results);
  };

  return (
    <Card style={{ padding: 18 }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 12, marginBottom: 12 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
            <div style={{ fontSize: 15, fontWeight: 700 }}>
              {t('settings.companion.title', 'Companion Pendant bring-up')}
            </div>
            <Pill tone="blue" size="sm">{t('settings.companion.badge', 'NUCLEO-WB55RG')}</Pill>
          </div>
          <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginTop: 5 }}>
            {t('settings.companion.desc', '先用开发板验证麦克风、外部 flash、OTA readiness 和按钮输入；正式产品工程从 Companion_Pendant.ioc 生成。')}
          </div>
        </div>
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(168px, 1fr))', gap: 8 }}>
        <CompanionFact
          label={t('settings.companion.sw1Label', 'SW1 / PC4')}
          value={t('settings.companion.sw1Value', '开始 / 停止录音')}
        />
        <CompanionFact
          label={t('settings.companion.sw2Label', 'SW2 / PD0')}
          value={t('settings.companion.sw2Value', '取消并回 idle')}
        />
        <CompanionFact
          label={t('settings.companion.sw3Label', 'SW3 / PD1')}
          value={t('settings.companion.sw3Value', 'flash + OTA + diag')}
        />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(190px, 1fr))', gap: 8, marginTop: 12 }}>
        <CompanionFact
          label={t('settings.companion.bringupProjectLabel', '当前硬件工程')}
          value="cubeide/Companion_Nucleo_WB55RG_Bringup"
          mono
        />
        <CompanionFact
          label={t('settings.companion.productIocLabel', '正式产品 .ioc')}
          value="Companion_Pendant.ioc"
          mono
        />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(156px, 1fr))', gap: 8, marginTop: 12 }}>
        <CompanionFact
          label={t('settings.companion.bleFixtureLabel', 'BLE fixture')}
          value={t('settings.companion.bleFixtureValue', 'VKA1 / companion')}
        />
        <CompanionFact
          label={t('settings.companion.a1Label', 'A1')}
          value={t('settings.companion.a1Value', 'fixture 短链路')}
        />
        <CompanionFact
          label={t('settings.companion.a2Label', 'A2')}
          value={t('settings.companion.a2Value', 'fixture 长窗口')}
        />
        <CompanionFact
          label={t('settings.companion.samePackageLabel', '同包')}
          value={t('settings.companion.samePackageValue', 'BLE OTA + 有线')}
        />
        <CompanionFact
          label={t('settings.companion.otaTransportLabel', 'OTA')}
          value="stm32wb_st_ble_ota"
          mono
        />
        <CompanionFact
          label={t('settings.companion.wiredTransportLabel', '有线')}
          value="ST-LINK / SWD"
          mono
        />
      </div>

      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 12, paddingTop: 12, borderTop: '0.5px solid var(--ol-line-soft)' }}>
        <Btn
          variant={run.action === 'probe' && run.status === 'pass' ? 'blue' : 'ghost'}
          size="sm"
          icon={busy && run.action === 'probe' ? 'refresh' : 'search'}
          onClick={() => void runAction('probe', () => probeEmbeddedAudioBleSubscription(COMPANION_PROBE_TIMEOUT_MS))}
          disabled={busy}
        >
          {busy && run.action === 'probe'
            ? t('settings.companion.bleProbeRunning', '探测中')
            : t('settings.companion.bleProbe', 'BLE 订阅')}
        </Btn>
        <Btn
          variant={run.action === 'a1' && run.status === 'pass' ? 'blue' : 'ghost'}
          size="sm"
          icon={busy && run.action === 'a1' ? 'refresh' : 'mic'}
          onClick={() => void runAction('a1', runCompanionA1Rounds)}
          disabled={busy}
        >
          {busy && run.action === 'a1'
            ? t('settings.companion.a1Running', 'A1 中')
            : t('settings.companion.a1Smoke', 'A1 smoke')}
        </Btn>
        <Btn
          variant={run.action === 'a2' && run.status === 'pass' ? 'blue' : 'ghost'}
          size="sm"
          icon={busy && run.action === 'a2' ? 'refresh' : 'clock'}
          onClick={() => void runAction('a2', () => submitCompanionEmbeddedAudioBleStream(COMPANION_A2_TIMEOUT_MS))}
          disabled={busy}
        >
          {busy && run.action === 'a2'
            ? t('settings.companion.a2Running', 'A2 中')
            : t('settings.companion.a2Smoke', 'A2 smoke')}
        </Btn>
        {commands.map(command => (
          <Btn
            key={command.id}
            variant={copied === command.id ? 'blue' : 'ghost'}
            size="sm"
            icon={copied === command.id ? 'check' : 'copy'}
            onClick={() => void copy(command.id, command.value)}
          >
            {copied === command.id ? t('common.copied') : command.label}
          </Btn>
        ))}
      </div>

      {run.status !== 'idle' && (
        <div style={{
          marginTop: 10,
          padding: '8px 10px',
          borderRadius: 7,
          border: '0.5px solid var(--ol-line-soft)',
          background: 'var(--ol-control-track)',
          display: 'flex',
          flexDirection: 'column',
          gap: 5,
        }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
            <Pill tone={companionRunTone(run.status)} size="sm">{companionRunLabel(run.status, t)}</Pill>
            <span style={{ fontSize: 11.5, color: run.status === 'fail' ? 'var(--ol-err)' : 'var(--ol-ink-3)', lineHeight: 1.45 }}>
              {run.message}
            </span>
          </div>
          {run.stats && (
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(132px, 1fr))', gap: 6 }}>
              <CompanionFact label={t('settings.companion.pcmBytes', 'PCM bytes')} value={String(run.stats.receivedPcmBytes)} mono />
              <CompanionFact label={t('settings.companion.packets', 'packets')} value={`${run.stats.receivedPacketCount}/${run.stats.expectedPacketCount ?? '-'}`} mono />
              <CompanionFact label={t('settings.companion.missingPackets', 'missing')} value={String(run.stats.missingPacketCount)} mono />
            </div>
          )}
        </div>
      )}
    </Card>
  );
}

function CompanionFact({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
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
      <div
        title={value}
        style={{
          fontSize: 11.5,
          color: 'var(--ol-ink)',
          fontFamily: mono ? 'var(--ol-font-mono)' : undefined,
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          whiteSpace: 'nowrap',
        }}
      >
        {value}
      </div>
    </div>
  );
}

function companionActionRunningLabel(action: CompanionAction, t: ReturnType<typeof useTranslation>['t']): string {
  if (action === 'probe') {
    return t('settings.companion.bleProbePending', '正在打开 Companion VKA1 notify。');
  }
  if (action === 'a1') {
    return t('settings.companion.a1Pending', '正在等待 fixture VKA1 短链路会话。');
  }
  return t('settings.companion.a2Pending', '正在等待 fixture VKA1 长窗口会话。');
}

function companionActionPassLabel(
  action: CompanionAction,
  result: EmbeddedAudioSubmissionResult | null,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (action === 'probe') {
    return t('settings.companion.bleProbePass', 'Companion VKA1 notify 已订阅。');
  }
  const stats = result?.stats;
  const pcm = stats?.receivedPcmBytes ?? 0;
  const missing = stats?.missingPacketCount ?? 0;
  if (action === 'a1') {
    return t(
      'settings.companion.a1Pass',
      'A1 {{rounds}} 轮完成：{{pcm}} bytes，missing={{missing}}。',
      { rounds: COMPANION_A1_ROUNDS, pcm, missing },
    );
  }
  return t(
    'settings.companion.a2Pass',
    'VKA1 会话完成：{{pcm}} bytes，missing={{missing}}。',
    { pcm, missing },
  );
}

function aggregateCompanionEmbeddedAudioResults(results: EmbeddedAudioSubmissionResult[]): EmbeddedAudioSubmissionResult {
  const stats = results.map(result => result.stats);
  const sum = (pick: (item: EmbeddedAudioSubmissionResult['stats']) => number) =>
    stats.reduce((total, item) => total + pick(item), 0);
  const expectedPacketCount = stats.every(item => typeof item.expectedPacketCount === 'number')
    ? sum(item => item.expectedPacketCount ?? 0)
    : null;
  const lastStats = stats[stats.length - 1];
  return {
    reconstructedPcmBytes: results.reduce((total, item) => total + item.reconstructedPcmBytes, 0),
    stats: {
      sessionId: null,
      explicitStartReceived: stats.every(item => item.explicitStartReceived),
      startInferredFromAudio: stats.some(item => item.startInferredFromAudio),
      terminalReceived: stats.every(item => item.terminalReceived),
      endReason: lastStats?.endReason ?? null,
      expectedPacketCount,
      receivedPacketCount: sum(item => item.receivedPacketCount),
      missingPacketCount: sum(item => item.missingPacketCount),
      missingPacketIndices: [],
      receivedPcmBytes: sum(item => item.receivedPcmBytes),
      reconstructedPcmBytes: sum(item => item.reconstructedPcmBytes),
      silenceFilledBytes: sum(item => item.silenceFilledBytes),
      duplicatePacketCount: sum(item => item.duplicatePacketCount),
      replacedPacketCount: sum(item => item.replacedPacketCount),
      ignoredForeignPacketCount: sum(item => item.ignoredForeignPacketCount),
      durationSeconds: sum(item => item.durationSeconds),
    },
  };
}

function companionRunTone(status: CompanionRunResult['status']): 'default' | 'blue' | 'ok' | 'err' {
  if (status === 'running') return 'blue';
  if (status === 'pass') return 'ok';
  if (status === 'fail') return 'err';
  return 'default';
}

function companionRunLabel(status: CompanionRunResult['status'], t: ReturnType<typeof useTranslation>['t']): string {
  if (status === 'running') return t('settings.companion.runRunning', '运行中');
  if (status === 'pass') return t('settings.companion.runPass', 'PASS');
  if (status === 'fail') return t('settings.companion.runFail', 'FAIL');
  return t('settings.companion.runIdle', 'idle');
}
