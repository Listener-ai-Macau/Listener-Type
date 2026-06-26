import { useEffect, useMemo, useState, type CSSProperties, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { ShortcutRecorder } from '../../components/ShortcutRecorder';
import { detectOS } from '../../components/WindowChrome';
import { SelectLite } from '../../components/ui/SelectLite';
import { defaultAppShortcutModifiers } from '../../lib/hotkey';
import {
  getDeviceSettings,
  getEmbeddedBleRuntimeStatus,
  listInstalledApplications,
  setDeviceSettings,
} from '../../lib/ipc';
import type {
  DeviceCustomKeyAction,
  DeviceCustomKeyAppPage,
  DeviceCustomKeyGesture,
  DeviceCustomKeyId,
  DeviceCustomKeys,
  DeviceCustomKeyMapping,
  DeviceKnobRotationAction,
  DeviceSettingsSnapshot,
  DeviceSettingsUpdateRequest,
  InstalledApplication,
  ShortcutBinding,
} from '../../lib/types';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { Btn, Card, Pill } from '../_atoms';
import { FirmwareOtaPanel } from './FirmwareOtaPanel';
import { inputStyle, SettingRow } from './shared';
import type { EmbeddedBleProbeStatus } from '../../components/EmbeddedBleStatusPanel';

const DEVICE_KEYS = [
  { id: 'key1' },
  { id: 'key2' },
  { id: 'key3' },
  { id: 'key4' },
] as const satisfies ReadonlyArray<{ id: DeviceCustomKeyId }>;

type DeviceKeyMapKey =
  | 'deviceCustomKeys'
  | 'deviceCustomKeyDoubleClicks'
  | 'deviceCustomKeyLongPresses';

const DEVICE_GESTURES: Array<{
  id: DeviceCustomKeyGesture;
  mapKey: DeviceKeyMapKey;
}> = [
  {
    id: 'singleClick',
    mapKey: 'deviceCustomKeys',
  },
  {
    id: 'doubleClick',
    mapKey: 'deviceCustomKeyDoubleClicks',
  },
  {
    id: 'longPress',
    mapKey: 'deviceCustomKeyLongPresses',
  },
];

const DEVICE_KEY_ACTIONS: DeviceCustomKeyAction[] = [
  'dictation',
  'openApp',
  'pasteShortcut',
  'openExternalApp',
  'copyShortcut',
  'undoShortcut',
  'sendShortcut',
  'pasteTemplate',
  'switchStyle',
  'translation',
  'selectionAsk',
  'disabled',
];

const DEVICE_KEY_APP_PAGES: DeviceCustomKeyAppPage[] = [
  'overview',
  'history',
  'vocab',
  'style',
  'translation',
  'selectionAsk',
  'settingsRecording',
  'settingsDevice',
  'settingsProviders',
  'settingsShortcuts',
  'settingsPermissions',
  'settingsLanguage',
  'settingsAdvanced',
];

const KNOB_ROTATION_ACTIONS: DeviceKnobRotationAction[] = ['systemVolume', 'screenBrightness', 'disabled'];

const KNOB_FIXED_ACTION_ROWS = [
  { gesture: 'doubleClick', action: 'bluetoothReset' },
  { gesture: 'longPress', action: 'powerOff' },
] as const;
const DEVICE_KEY_LABEL_WIDTH = 50;
const DEVICE_KEY_LABEL_GAP = 16;
const DEVICE_KEY_ROW_TEMPLATE = `${DEVICE_KEY_LABEL_WIDTH}px minmax(0, 1fr)`;
const DEVICE_KEY_CONTROL_GAP = 8;
const DEVICE_KEY_MAIN_CONTROL_WIDTH = 208;
const DEVICE_KEY_SECONDARY_CONTROL_WIDTH = 208;
const DEVICE_KEY_DETAIL_WIDTH = DEVICE_KEY_MAIN_CONTROL_WIDTH + DEVICE_KEY_CONTROL_GAP + DEVICE_KEY_SECONDARY_CONTROL_WIDTH;
const KNOB_CONTROL_WIDTH = 208;

const EXTERNAL_APP_MANUAL_VALUE = '__manual_external_app__';
const DEVICE_SETTINGS_REFRESH_MS = 8000;

const fallbackShortcut = (): ShortcutBinding => ({
  primary: 'K',
  modifiers: defaultAppShortcutModifiers(),
});

export function DeviceSection() {
  const { t } = useTranslation();
  const { prefs, updatePrefs: savePrefs } = useHotkeySettings();
  const [installedApps, setInstalledApps] = useState<InstalledApplication[]>([]);
  const [installedAppsLoading, setInstalledAppsLoading] = useState(false);
  const [bleStatus, setBleStatus] = useState<EmbeddedBleProbeStatus>('idle');
  const bleSupported = detectOS() === 'win';
  const autoOpenDeviceKeyActionMenu =
    import.meta.env.DEV &&
    new URLSearchParams(window.location.search).get('openDeviceKeyActionMenu') === '1';

  useEffect(() => {
    let cancelled = false;
    setInstalledAppsLoading(true);
    listInstalledApplications()
      .then(apps => {
        if (!cancelled) setInstalledApps(apps);
      })
      .catch(error => {
        console.warn('[device-key] list installed applications failed', error);
        if (!cancelled) setInstalledApps([]);
      })
      .finally(() => {
        if (!cancelled) setInstalledAppsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    const refresh = async () => {
      if (!bleSupported) {
        setBleStatus('error');
        return;
      }
      setBleStatus('checking');
      try {
        const status = await getEmbeddedBleRuntimeStatus();
        if (!cancelled) {
          setBleStatus(status.backgroundListenerReady ? 'ok' : 'idle');
        }
      } catch (error) {
        console.warn('[device] embedded BLE runtime status failed', error);
        if (!cancelled) setBleStatus('error');
      }
    };
    void refresh();
    const timer = window.setInterval(refresh, DEVICE_SETTINGS_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [bleSupported]);

  if (!prefs) {
    return (
      <Card>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
      </Card>
    );
  }

  const knobRotationAction = prefs.deviceKnobRotationAction ?? 'systemVolume';
  const updateKnobRotationAction = async (value: string) => {
    await savePrefs(current => ({
      ...current,
      deviceKnobRotationAction: value as DeviceKnobRotationAction,
    }));
  };

  return (
    <>
      <DeviceFirmwareSettingsCard />

      <FirmwareOtaPanel
        supported={bleSupported}
        bleStatus={bleStatus}
      />

      <Card>
        <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>
          {t('settings.deviceKeys.title')}
        </div>
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, marginBottom: 2 }}>
          {t('settings.deviceKeys.desc')}
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 14, marginTop: 12 }}>
          {DEVICE_GESTURES.map(gesture => (
            <DeviceKeyGestureGroup
              key={gesture.id}
              gesture={gesture}
              keys={prefs[gesture.mapKey]}
              installedApps={installedApps}
              installedAppsLoading={installedAppsLoading}
              autoOpenDeviceKeyActionMenu={autoOpenDeviceKeyActionMenu}
              onChange={async (id, mapping) => {
                await savePrefs(current => ({
                  ...current,
                  [gesture.mapKey]: {
                    ...current[gesture.mapKey],
                    [id]: mapping,
                  },
                }));
              }}
            />
          ))}
        </div>

        <KnobActionsPanel
          knobClickMapping={prefs.deviceCustomKeys.knob}
          knobRotationAction={knobRotationAction}
          installedApps={installedApps}
          installedAppsLoading={installedAppsLoading}
          onKnobClickMappingChange={async mapping => {
            await savePrefs(current => ({
              ...current,
              deviceCustomKeys: {
                ...current.deviceCustomKeys,
                knob: mapping,
              },
            }));
          }}
          onKnobRotationActionChange={updateKnobRotationAction}
        />
      </Card>

      <CompanionBringupCard />
    </>
  );
}

function CompanionBringupCard() {
  const { t } = useTranslation();
  const [copied, setCopied] = useState('');
  const commands = [
    {
      id: 'build',
      label: t('settings.companion.buildCommand', '编译 bring-up'),
      value: 'pwsh -NoProfile -File .\\tools\\build_nucleo_bringup.ps1',
    },
    {
      id: 'flash',
      label: t('settings.companion.flashCommand', '刷入 NUCLEO'),
      value: 'pwsh -NoProfile -File .\\tools\\flash_nucleo_bringup.ps1',
    },
    {
      id: 'capture',
      label: t('settings.companion.captureCommand', '抓串口日志'),
      value: 'pwsh -NoProfile -File .\\tools\\capture_nucleo_serial.ps1 -Port COM13 -Seconds 30',
    },
  ];
  const copy = async (id: string, value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(id);
      window.setTimeout(() => setCopied(current => (current === id ? '' : current)), 1400);
    } catch (error) {
      console.warn('[companion] copy command failed', error);
    }
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

      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 12, paddingTop: 12, borderTop: '0.5px solid var(--ol-line-soft)' }}>
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

function DeviceFirmwareSettingsCard() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<DeviceSettingsSnapshot | null>(null);
  const [form, setForm] = useState<DeviceSettingsUpdateRequest>({
    statusLedBrightnessPercent: 100,
    keyLedBrightnessPercent: 100,
    knobLedBrightnessPercent: 100,
    edgeLedBrightnessPercent: 100,
    pluggedLowPowerIdleMinutes: 1,
    batteryLowPowerIdleMinutes: 1,
    pluggedLowPowerEnabled: true,
    pluggedAutoShutdownMinutes: 0,
    batteryAutoShutdownMinutes: 30,
    bleName: 'listener',
  });
  const [status, setStatus] = useState<'idle' | 'loading' | 'saving' | 'saved' | 'error'>('loading');
  const [message, setMessage] = useState('');

  const refresh = async () => {
    setStatus(previous => (previous === 'saving' ? previous : 'loading'));
    setMessage('');
    try {
      const value = await getDeviceSettings();
      setSnapshot(value);
      setForm(snapshotToForm(value));
      setStatus('idle');
    } catch (error) {
      setStatus('error');
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const validationError = validateDeviceSettingsForm(form, t);
  const writeDisabled = status === 'loading' || status === 'saving' || !!validationError || !snapshot?.writeSupported;
  const readDisabled = status === 'loading' || status === 'saving';
  const controlsDisabled = !snapshot?.writeSupported || status === 'saving';
  const ledControlsDisabled = controlsDisabled || !(snapshot?.ledZoneBrightnessSupported ?? false);

  const save = async () => {
    if (writeDisabled) return;
    setStatus('saving');
    setMessage('');
    try {
      const value = await setDeviceSettings({
        ...form,
        pluggedLowPowerEnabled: form.pluggedLowPowerIdleMinutes > 0,
        pluggedAutoShutdownMinutes: 0,
      });
      setSnapshot(value);
      setForm(snapshotToForm(value));
      setStatus('saved');
      setMessage(t('settings.device.configSaved', '已发送到设备'));
      window.setTimeout(() => setStatus(current => (current === 'saved' ? 'idle' : current)), 1800);
    } catch (error) {
      setStatus('error');
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };
  const detailText = formatDeviceSnapshotDetail(snapshot, t);
  const footerText = validationError || message || formatDeviceSnapshotFooter(snapshot, t);

  return (
    <Card className="ol-device-settings-card" style={{ padding: 20 }}>
      <div className="ol-device-settings-header" style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) auto', alignItems: 'center', gap: 12, marginBottom: 12 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 15, fontWeight: 700 }}>
            {t('settings.device.configTitle', '设备设置')}
          </div>
        </div>
        <div className="ol-device-settings-toolbar" style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', justifyContent: 'center' }}>
          <div className="ol-device-readwrite-row" style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 8, width: 'min(100%, 176px)' }}>
            <Btn variant="ghost" size="sm" icon="refresh" onClick={() => void refresh()} disabled={readDisabled} style={{ height: 34, justifyContent: 'center', minWidth: 0, whiteSpace: 'nowrap' }}>
              {status === 'loading'
                ? t('settings.device.reading', '读取中')
                : t('settings.device.readFromDevice', '读取')}
            </Btn>
            <Btn variant="blue" size="sm" icon="check" disabled={writeDisabled} onClick={() => void save()} style={{ height: 34, justifyContent: 'center', minWidth: 0, whiteSpace: 'nowrap' }}>
              {status === 'saving'
                ? t('settings.device.writing', '写入中')
                : t('settings.device.writeToDevice', '写入')}
            </Btn>
          </div>
        </div>
      </div>

      <DeviceSettingsStatusStrip snapshot={snapshot} t={t} />

      <DeviceSettingsPanel
        title={t('settings.device.bleNameLabel', '蓝牙名称')}
        desc={t('settings.device.bleNameDesc', '1-32 个 ASCII 字符；部分 Windows 设备名变更需要重连或重新配对后才显示。')}
      >
        <div className="ol-device-ble-name-card">
          <input
            value={form.bleName}
            maxLength={32}
            disabled={controlsDisabled}
            onChange={event => setForm(current => ({ ...current, bleName: event.target.value }))}
            style={{ ...inputStyle, flex: '0 1 320px', maxWidth: 320 }}
          />
        </div>
      </DeviceSettingsPanel>

      {detailText && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginTop: 8 }}>
          {detailText}
        </div>
      )}

      <DeviceSettingsPanel
        title={t('settings.device.powerTimingTitle', '电源时间')}
        desc={t('settings.device.powerTimingDesc', '插电和电池模式可分别设置低功耗等待与自动关机。')}
      >
        <div className="ol-device-power-timing-grid" style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(260px, 1fr))', gap: 10 }}>
          <PowerModeTimingGroup
            title={t('settings.device.pluggedModeTitle', '插电模式')}
            desc={t('settings.device.pluggedModeDesc', 'USB、充电或外部供电时使用。')}
            lowPowerLabel={t('settings.device.pluggedLowPowerIdleLabel', '进入低功耗')}
            lowPowerDesc={t('settings.device.pluggedLowPowerIdleDesc', '无操作多久后进入低功耗空闲。')}
            lowPowerValue={form.pluggedLowPowerIdleMinutes}
            disabled={controlsDisabled}
            onLowPowerChange={value => setForm(current => ({
              ...current,
              pluggedLowPowerIdleMinutes: value,
              pluggedLowPowerEnabled: value > 0,
            }))}
            t={t}
          />
          <PowerModeTimingGroup
            title={t('settings.device.batteryModeTitle', '电池模式')}
            desc={t('settings.device.batteryModeDesc', '拔电后使用；低功耗会灭灯。')}
            lowPowerLabel={t('settings.device.batteryLowPowerIdleLabel', '进入低功耗')}
            lowPowerDesc={t('settings.device.batteryLowPowerIdleDesc', '无操作多久进入低功耗空闲。')}
            lowPowerValue={form.batteryLowPowerIdleMinutes}
            autoShutdownLabel={t('settings.device.batteryAutoShutdownLabel', '自动关机')}
            autoShutdownDesc={t('settings.device.batteryAutoShutdownDesc', '电池供电且长时间空闲时自动关机。')}
            autoShutdownValue={form.batteryAutoShutdownMinutes}
            disabled={controlsDisabled}
            onLowPowerChange={value => setForm(current => ({ ...current, batteryLowPowerIdleMinutes: value }))}
            onAutoShutdownChange={value => setForm(current => ({ ...current, batteryAutoShutdownMinutes: value }))}
            t={t}
          />
        </div>
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 8, lineHeight: 1.4 }}>
          {t('settings.device.zeroMeansOff', '0 = 关闭')}
        </div>
      </DeviceSettingsPanel>

      <DeviceLedBrightnessGroup
        supported={snapshot?.ledZoneBrightnessSupported ?? false}
        disabled={ledControlsDisabled}
        values={form}
        onChange={(field, value) => setForm(current => ({ ...current, [field]: value }))}
        t={t}
      />

      {footerText && (
        <div style={{ fontSize: 11.5, color: validationError || status === 'error' ? 'var(--ol-err)' : status === 'saved' ? 'var(--ol-ok)' : 'var(--ol-ink-4)', lineHeight: 1.45, paddingTop: 12, borderTop: '0.5px solid var(--ol-line-soft)' }}>
          {footerText}
        </div>
      )}
    </Card>
  );
}

function DeviceSettingsPanel({
  title,
  desc,
  children,
}: {
  title?: string;
  desc?: string;
  children: ReactNode;
}) {
  return (
    <section className="ol-device-settings-panel">
      {title && (
        <div className="ol-device-settings-panel-header">
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 12.5, fontWeight: 700 }}>{title}</div>
            {desc && (
              <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 2, lineHeight: 1.45 }}>{desc}</div>
            )}
          </div>
        </div>
      )}
      {children}
    </section>
  );
}

function DeviceSettingsStatusStrip({
  snapshot,
  t,
}: {
  snapshot: DeviceSettingsSnapshot | null;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  const items = getDeviceSettingsStatusItems(snapshot, t);
  return (
    <div className="ol-device-status-strip">
      {items.map(item => (
        <span key={item.label} className="ol-device-status-chip" title={`${item.label}: ${item.value}`}>
          <span className="ol-device-status-label">{item.label}</span>
          <span className="ol-device-status-value">{item.value}</span>
        </span>
      ))}
    </div>
  );
}

function PowerModeTimingGroup({
  title,
  desc,
  lowPowerLabel,
  lowPowerDesc,
  lowPowerValue,
  autoShutdownLabel,
  autoShutdownDesc,
  autoShutdownValue,
  disabled,
  onLowPowerChange,
  onAutoShutdownChange,
  t,
}: {
  title: string;
  desc: string;
  lowPowerLabel: string;
  lowPowerDesc: string;
  lowPowerValue: number;
  autoShutdownLabel?: string;
  autoShutdownDesc?: string;
  autoShutdownValue?: number;
  disabled: boolean;
  onLowPowerChange: (value: number) => void;
  onAutoShutdownChange?: (value: number) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <div className="ol-device-power-mode-card">
      <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 10, marginBottom: 8 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 12.5, fontWeight: 700 }}>{title}</div>
          <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 2, lineHeight: 1.45 }}>{desc}</div>
        </div>
      </div>
      <TimingControl
        label={lowPowerLabel}
        desc={lowPowerDesc}
        value={lowPowerValue}
        min={0}
        disabled={disabled}
        onChange={onLowPowerChange}
        t={t}
      />
      {autoShutdownLabel && autoShutdownDesc && typeof autoShutdownValue === 'number' && onAutoShutdownChange && (
        <TimingControl
          label={autoShutdownLabel}
          desc={autoShutdownDesc}
          value={autoShutdownValue}
          min={0}
          disabled={disabled}
          onChange={onAutoShutdownChange}
          t={t}
        />
      )}
    </div>
  );
}

function TimingControl({
  label,
  desc,
  value,
  min,
  disabled,
  onChange,
  t,
}: {
  label: string;
  desc: string;
  value: number;
  min: 0 | 1;
  disabled: boolean;
  onChange: (value: number) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <div className="ol-device-timing-control" style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) minmax(132px, 0.72fr)', gap: 12, alignItems: 'center', padding: '9px 0', borderTop: '0.5px solid var(--ol-line-soft)' }}>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontSize: 12.5, fontWeight: 600 }}>{label}</div>
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 2, lineHeight: 1.4 }}>{desc}</div>
      </div>
      <MinuteInput value={value} min={min} disabled={disabled} onChange={onChange} t={t} />
    </div>
  );
}

function MinuteInput({
  value,
  min,
  disabled,
  onChange,
  t,
}: {
  value: number;
  min: 0 | 1;
  disabled: boolean;
  onChange: (value: number) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <div className="ol-device-minute-input">
      <input
        type="number"
        min={min}
        max={1440}
        value={value}
        disabled={disabled}
        onChange={event => {
          const next = Number(event.target.value);
          onChange(Number.isFinite(next) ? next : value);
        }}
        style={{ ...inputStyle, flex: '0 1 96px', maxWidth: 120 }}
      />
      <span style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>
        {t('settings.device.minutes', '分钟')}
      </span>
    </div>
  );
}

type LedBrightnessField =
  | 'statusLedBrightnessPercent'
  | 'keyLedBrightnessPercent'
  | 'knobLedBrightnessPercent'
  | 'edgeLedBrightnessPercent';

function DeviceLedBrightnessGroup({
  supported,
  disabled,
  values,
  onChange,
  t,
}: {
  supported: boolean;
  disabled: boolean;
  values: Pick<DeviceSettingsUpdateRequest, LedBrightnessField>;
  onChange: (field: LedBrightnessField, value: number) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <DeviceSettingsPanel
      title={t('settings.device.ledZoneTitle', '灯区亮度')}
      desc={supported
        ? t('settings.device.ledZoneDesc', '按灯区限制最大亮度，写入后同步到设备。')
        : t('settings.device.ledZoneUnsupported', '当前固件未回读四区亮度；更新固件后可写入。')}
    >
      <div className="ol-device-led-grid">
        <LedBrightnessControl
          label={t('settings.device.statusLedBrightnessLabel', '状态灯')}
          desc={t('settings.device.statusLedBrightnessDesc', 'PWR / BLE / REC / AI / OK / WARN')}
          value={values.statusLedBrightnessPercent}
          disabled={disabled}
          onChange={value => onChange('statusLedBrightnessPercent', value)}
        />
        <LedBrightnessControl
          label={t('settings.device.keyLedBrightnessLabel', '按键灯')}
          desc={t('settings.device.keyLedBrightnessDesc', 'KEY1-KEY4')}
          value={values.keyLedBrightnessPercent}
          disabled={disabled}
          onChange={value => onChange('keyLedBrightnessPercent', value)}
        />
        <LedBrightnessControl
          label={t('settings.device.knobLedBrightnessLabel', '旋钮灯')}
          desc={t('settings.device.knobLedBrightnessDesc', 'EC11 环灯')}
          value={values.knobLedBrightnessPercent}
          disabled={disabled}
          onChange={value => onChange('knobLedBrightnessPercent', value)}
        />
        <LedBrightnessControl
          label={t('settings.device.edgeLedBrightnessLabel', '板框灯')}
          desc={t('settings.device.edgeLedBrightnessDesc', '边框氛围灯')}
          value={values.edgeLedBrightnessPercent}
          disabled={disabled}
          onChange={value => onChange('edgeLedBrightnessPercent', value)}
        />
      </div>
    </DeviceSettingsPanel>
  );
}

function LedBrightnessControl({
  label,
  desc,
  value,
  disabled,
  onChange,
}: {
  label: string;
  desc: string;
  value: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  return (
    <div className="ol-device-led-row">
      <div className="ol-device-led-meta">
        <div style={{ minWidth: 0 }}>
          <div className="ol-device-led-title">{label}</div>
          <div className="ol-device-led-desc">{desc}</div>
        </div>
      </div>
      <PercentSlider value={value} disabled={disabled} onChange={onChange} />
      <PercentNumber value={value} disabled={disabled} onChange={onChange} />
    </div>
  );
}

function PercentNumber({
  value,
  disabled,
  onChange,
}: {
  value: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  const safeValue = clampPercent(value);
  return (
    <div className="ol-device-percent-number">
      <input
        type="number"
        min={0}
        max={100}
        value={safeValue}
        disabled={disabled}
        onChange={event => onChange(clampPercent(Number(event.target.value)))}
        style={{ ...inputStyle, width: 56, height: 26, flex: '0 0 56px', maxWidth: 56, padding: '0 6px', textAlign: 'right' }}
      />
      <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)' }}>%</span>
    </div>
  );
}

function PercentSlider({
  value,
  disabled,
  onChange,
}: {
  value: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  const safeValue = clampPercent(value);
  return (
    <div className="ol-device-percent-slider" style={{ display: 'flex', alignItems: 'center', width: '100%', minWidth: 0 }}>
      <input
        className="ol-device-percent-range"
        type="range"
        min={0}
        max={100}
        value={safeValue}
        disabled={disabled}
        onChange={event => onChange(clampPercent(Number(event.target.value)))}
        style={{ width: '100%', minWidth: 0, accentColor: 'var(--ol-blue)', '--ol-device-range-percent': `${safeValue}%` } as CSSProperties}
      />
    </div>
  );
}

function KnobActionsPanel({
  knobClickMapping,
  knobRotationAction,
  installedApps,
  installedAppsLoading,
  onKnobClickMappingChange,
  onKnobRotationActionChange,
}: {
  knobClickMapping: DeviceCustomKeyMapping;
  knobRotationAction: DeviceKnobRotationAction;
  installedApps: InstalledApplication[];
  installedAppsLoading: boolean;
  onKnobClickMappingChange: (mapping: DeviceCustomKeyMapping) => Promise<void>;
  onKnobRotationActionChange: (value: string) => Promise<void>;
}) {
  const { t } = useTranslation();

  return (
    <>
      <div style={{ fontSize: 13, fontWeight: 600, marginTop: 18, paddingTop: 14, borderTop: '0.5px solid var(--ol-line-soft)' }}>
        {t('settings.deviceKeys.knob.title')}
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, marginBottom: 8 }}>
        {t('settings.deviceKeys.knob.desc')}
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        <div
          className="ol-device-knob-row"
          style={{
            display: 'grid',
            gridTemplateColumns: DEVICE_KEY_ROW_TEMPLATE,
            columnGap: DEVICE_KEY_LABEL_GAP,
            alignItems: 'center',
          }}
        >
          <div style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('settings.deviceKeys.knob.gestures.rotate')}
          </div>
          <SelectLite
            value={knobRotationAction}
            onChange={value => void onKnobRotationActionChange(value)}
            options={KNOB_ROTATION_ACTIONS.map(action => ({
              value: action,
              label: t(`settings.deviceKeys.knob.actions.${action}`),
            }))}
            style={{ ...inputStyle, flex: `0 1 ${KNOB_CONTROL_WIDTH}px`, width: KNOB_CONTROL_WIDTH, maxWidth: '100%', minWidth: 0 }}
            ariaLabel={t('settings.deviceKeys.knob.rotationActionSelectAria')}
          />
        </div>
        <div
          className="ol-device-knob-row"
          style={{
            display: 'grid',
            gridTemplateColumns: DEVICE_KEY_ROW_TEMPLATE,
            columnGap: DEVICE_KEY_LABEL_GAP,
            alignItems: 'start',
          }}
        >
          <div style={{ height: 32, display: 'flex', alignItems: 'center', fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('settings.deviceKeys.knob.gestures.shortPress')}
          </div>
          <DeviceKeyMappingControl
            mapping={knobClickMapping}
            installedApps={installedApps}
            installedAppsLoading={installedAppsLoading}
            actionWidthOverride={KNOB_CONTROL_WIDTH}
            onChange={onKnobClickMappingChange}
          />
        </div>
        {KNOB_FIXED_ACTION_ROWS.map(item => (
          <div
            key={item.gesture}
            className="ol-device-knob-row"
            style={{
              display: 'grid',
              gridTemplateColumns: DEVICE_KEY_ROW_TEMPLATE,
              columnGap: DEVICE_KEY_LABEL_GAP,
              alignItems: 'center',
              opacity: 0.66,
            }}
          >
            <div style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink-3)' }}>
              {t(`settings.deviceKeys.knob.gestures.${item.gesture}`)}
            </div>
            <div
              aria-disabled
              style={{
                minHeight: 32,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: 8,
                width: KNOB_CONTROL_WIDTH,
                maxWidth: '100%',
                boxSizing: 'border-box',
                padding: '0 10px',
                borderRadius: 6,
                background: 'var(--ol-surface-2)',
                border: '0.5px solid var(--ol-line-strong)',
                color: 'var(--ol-ink-4)',
                fontSize: 12,
              }}
            >
              <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                {t(`settings.deviceKeys.knob.actions.${item.action}`)}
              </span>
            </div>
          </div>
        ))}
      </div>
    </>
  );
}

function DeviceKeyGestureGroup({
  gesture,
  keys,
  installedApps,
  installedAppsLoading,
  autoOpenDeviceKeyActionMenu,
  onChange,
}: {
  gesture: (typeof DEVICE_GESTURES)[number];
  keys: DeviceCustomKeys;
  installedApps: InstalledApplication[];
  installedAppsLoading: boolean;
  autoOpenDeviceKeyActionMenu: boolean;
  onChange: (id: DeviceCustomKeyId, mapping: DeviceCustomKeyMapping) => Promise<void>;
}) {
  const { t } = useTranslation();
  return (
    <div style={{ borderTop: '0.5px solid var(--ol-line-soft)', paddingTop: 12 }}>
      <div style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)', marginBottom: 8 }}>
        {t(`settings.deviceKeys.gestures.${gesture.id}`)}
      </div>
      <div style={{ display: 'flex', flexDirection: 'column' }}>
        {DEVICE_KEYS.map(({ id }, index) => (
          <div
            key={`${gesture.id}-${id}`}
            className="ol-device-key-row"
            style={{
              display: 'grid',
              gridTemplateColumns: DEVICE_KEY_ROW_TEMPLATE,
              columnGap: DEVICE_KEY_LABEL_GAP,
              alignItems: 'start',
              minWidth: 0,
              padding: index === 0 ? '4px 0 8px' : '9px 0 8px',
              borderTop: index === 0 ? 'none' : '0.5px solid var(--ol-line-soft)',
            }}
          >
            <div
              className="ol-device-key-label-cell"
              style={{
                minWidth: 0,
                height: 32,
                display: 'flex',
                alignItems: 'center',
              }}
            >
              <div
                className="ol-device-key-label"
                style={{
                  color: 'var(--ol-ink)',
                  fontSize: 12.5,
                  fontWeight: 700,
                  lineHeight: 1,
                }}
              >
                {t('settings.deviceKeys.keyLabel', { key: id.toUpperCase() })}
              </div>
            </div>
            <DeviceKeyMappingControl
              mapping={keys[id]}
              installedApps={installedApps}
              installedAppsLoading={installedAppsLoading}
              autoOpen={autoOpenDeviceKeyActionMenu && gesture.id === 'singleClick' && id === 'key1'}
              onChange={mapping => onChange(id, mapping)}
            />
          </div>
        ))}
      </div>
    </div>
  );
}

function DeviceKeyMappingControl({
  mapping,
  installedApps,
  installedAppsLoading,
  autoOpen = false,
  actionWidthOverride,
  onChange,
}: {
  mapping: DeviceCustomKeyMapping;
  installedApps: InstalledApplication[];
  installedAppsLoading: boolean;
  autoOpen?: boolean;
  actionWidthOverride?: number;
  onChange: (mapping: DeviceCustomKeyMapping) => Promise<void>;
}) {
  const { t } = useTranslation();
  const externalAppPath = mapping.externalAppPath ?? '';
  const matchingInstalledApp = useMemo(
    () => installedApps.find(app => app.path.toLocaleLowerCase() === externalAppPath.toLocaleLowerCase()),
    [externalAppPath, installedApps],
  );
  const externalAppOptions = useMemo(() => [
    {
      value: EXTERNAL_APP_MANUAL_VALUE,
      label: installedAppsLoading
        ? t('settings.deviceKeys.installedAppLoading')
        : t('settings.deviceKeys.installedAppManual'),
    },
    ...installedApps.map(app => ({
      value: app.path,
      label: app.name,
    })),
  ], [installedApps, installedAppsLoading, t]);
  const externalAppPickerValue = matchingInstalledApp?.path ?? EXTERNAL_APP_MANUAL_VALUE;
  const mainActionWidth = actionWidthOverride ?? DEVICE_KEY_MAIN_CONTROL_WIDTH;
  const controlBaseStyle = {
    ...inputStyle,
    height: 32,
    padding: '0 9px',
    borderRadius: 8,
    boxSizing: 'border-box' as const,
    maxWidth: '100%',
  };
  const detailInputStyle = {
    ...controlBaseStyle,
    height: 27,
    fontSize: 11.5,
    fontFamily: 'var(--ol-font-mono)',
    color: 'var(--ol-ink-3)',
    background: 'transparent',
    borderColor: 'var(--ol-line-soft)',
  };

  const updateAction = async (action: DeviceCustomKeyAction) => {
    await onChange({
      ...mapping,
      action,
      shortcut: action === 'sendShortcut' ? mapping.shortcut ?? fallbackShortcut() : mapping.shortcut,
      appPage: action === 'openApp' ? mapping.appPage ?? 'settingsDevice' : mapping.appPage,
      externalAppPath,
    });
  };

  return (
    <div
      className="ol-device-key-control-stack"
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
        alignItems: 'flex-start',
        width: '100%',
        maxWidth: '100%',
        minWidth: 0,
      }}
    >
      <div
        className="ol-device-key-action-row"
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: DEVICE_KEY_CONTROL_GAP,
          alignItems: 'center',
          width: '100%',
          maxWidth: '100%',
          minWidth: 0,
        }}
      >
        <SelectLite
          value={mapping.action}
          onChange={value => void updateAction(value as DeviceCustomKeyAction)}
          options={DEVICE_KEY_ACTIONS.map(action => ({
            value: action,
            label: t(`settings.deviceKeys.actions.${action}`),
          }))}
          defaultOpen={autoOpen}
          style={{ ...controlBaseStyle, flex: `0 1 ${mainActionWidth}px`, width: mainActionWidth, maxWidth: '100%', minWidth: 0 }}
          ariaLabel={t('settings.deviceKeys.actionSelectAria')}
        />
        {mapping.action === 'openApp' && (
          <SelectLite
            value={mapping.appPage ?? 'settingsDevice'}
            onChange={value => void onChange({ ...mapping, appPage: value as DeviceCustomKeyAppPage })}
            options={DEVICE_KEY_APP_PAGES.map(page => ({
              value: page,
              label: t(`settings.deviceKeys.appPages.${page}`),
            }))}
            style={{ ...controlBaseStyle, flex: `0 1 ${DEVICE_KEY_SECONDARY_CONTROL_WIDTH}px`, width: DEVICE_KEY_SECONDARY_CONTROL_WIDTH, maxWidth: '100%', minWidth: 0 }}
            ariaLabel={t('settings.deviceKeys.appPageSelectAria')}
          />
        )}
        {mapping.action === 'openExternalApp' && (
          <SelectLite
            value={externalAppPickerValue}
            onChange={value => {
              if (value === EXTERNAL_APP_MANUAL_VALUE) return;
              void onChange({ ...mapping, externalAppPath: value });
            }}
            options={externalAppOptions}
            style={{ ...controlBaseStyle, flex: `0 1 ${DEVICE_KEY_SECONDARY_CONTROL_WIDTH}px`, width: DEVICE_KEY_SECONDARY_CONTROL_WIDTH, maxWidth: '100%', minWidth: 0 }}
            ariaLabel={t('settings.deviceKeys.installedAppSelectAria')}
          />
        )}
      </div>
      {mapping.action === 'openExternalApp' && (
        <input
          value={externalAppPath}
          onChange={event => {
            const externalAppPath = event.target.value;
            void onChange({ ...mapping, externalAppPath });
          }}
          placeholder={
            installedApps.length === 0 && !installedAppsLoading
              ? t('settings.deviceKeys.installedAppEmpty')
              : t('settings.deviceKeys.externalAppPlaceholder')
          }
          title={externalAppPath}
          style={{ ...detailInputStyle, width: '100%', maxWidth: DEVICE_KEY_DETAIL_WIDTH, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}
        />
      )}
      {mapping.action === 'pasteTemplate' && (
        <input
          value={mapping.pasteTemplate}
          onChange={event => {
            const pasteTemplate = event.target.value;
            void onChange({ ...mapping, pasteTemplate });
          }}
          placeholder={t('settings.deviceKeys.templatePlaceholder')}
          style={{ ...detailInputStyle, width: '100%', maxWidth: DEVICE_KEY_DETAIL_WIDTH, minWidth: 0 }}
        />
      )}
      {mapping.action === 'sendShortcut' && (
        <div style={{ width: 240, maxWidth: '100%', minWidth: 0 }}>
          <ShortcutRecorder
            value={mapping.shortcut ?? fallbackShortcut()}
            alignRecordButton
            onSave={async shortcut => {
              await onChange({ ...mapping, shortcut });
            }}
          />
        </div>
      )}
    </div>
  );
}

function snapshotToForm(snapshot: DeviceSettingsSnapshot): DeviceSettingsUpdateRequest {
  return {
    statusLedBrightnessPercent: snapshot.statusLedBrightnessPercent,
    keyLedBrightnessPercent: snapshot.keyLedBrightnessPercent,
    knobLedBrightnessPercent: snapshot.knobLedBrightnessPercent,
    edgeLedBrightnessPercent: snapshot.edgeLedBrightnessPercent,
    pluggedLowPowerIdleMinutes: snapshot.pluggedLowPowerEnabled ? snapshot.pluggedLowPowerIdleMinutes : 0,
    batteryLowPowerIdleMinutes: snapshot.batteryLowPowerIdleMinutes,
    pluggedLowPowerEnabled: snapshot.pluggedLowPowerEnabled,
    pluggedAutoShutdownMinutes: 0,
    batteryAutoShutdownMinutes: Math.round(snapshot.batteryAutoShutdownMs / 60000),
    bleName: snapshot.bleName,
  };
}

function validateDeviceSettingsForm(
  form: DeviceSettingsUpdateRequest,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (
    !Number.isFinite(form.statusLedBrightnessPercent) || form.statusLedBrightnessPercent < 0 || form.statusLedBrightnessPercent > 100 ||
    !Number.isFinite(form.keyLedBrightnessPercent) || form.keyLedBrightnessPercent < 0 || form.keyLedBrightnessPercent > 100 ||
    !Number.isFinite(form.knobLedBrightnessPercent) || form.knobLedBrightnessPercent < 0 || form.knobLedBrightnessPercent > 100 ||
    !Number.isFinite(form.edgeLedBrightnessPercent) || form.edgeLedBrightnessPercent < 0 || form.edgeLedBrightnessPercent > 100
  ) {
    return t('settings.device.errorZoneBrightness', '灯区亮度必须在 0-100 之间。');
  }
  if (!Number.isFinite(form.pluggedLowPowerIdleMinutes) || form.pluggedLowPowerIdleMinutes < 0 || form.pluggedLowPowerIdleMinutes > 1440) {
    return t('settings.device.errorLowPowerIdle', '低功耗等待时间必须在 0-1440 分钟之间。');
  }
  if (!Number.isFinite(form.batteryLowPowerIdleMinutes) || form.batteryLowPowerIdleMinutes < 0 || form.batteryLowPowerIdleMinutes > 1440) {
    return t('settings.device.errorLowPowerIdle', '低功耗等待时间必须在 0-1440 分钟之间。');
  }
  if (!Number.isFinite(form.batteryAutoShutdownMinutes) || form.batteryAutoShutdownMinutes < 0 || form.batteryAutoShutdownMinutes > 1440) {
    return t('settings.device.errorAutoShutdown', '自动关机时间必须在 0-1440 分钟之间。');
  }
  if (!isValidBleName(form.bleName)) {
    return t('settings.device.errorBleName', '蓝牙名称必须是 1-32 个可打印 ASCII 字符，不能包含空格、引号、分号、等号或反斜杠。');
  }
  return '';
}

function formatDeviceSnapshotDetail(
  snapshot: DeviceSettingsSnapshot | null,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (!snapshot?.detail) return '';
  if (snapshot.source === 'lastKnown') {
    return t('settings.device.detailLastKnown', '已写入设备；本次读取回执暂时不可用，界面显示刚刚写入的值。');
  }
  if (snapshot.source === 'defaults' && snapshot.connected) {
    return t('settings.device.detailDefaults', '设备已连接，但暂时读不到配置，当前显示默认值。');
  }
  if (snapshot.source === 'unavailable') {
    return t('settings.device.detailUnavailable', '设备未连接；连接后可以读取和写入配置。');
  }
  return '';
}

function isValidBleName(value: string): boolean {
  if (value.length < 1 || value.length > 32) return false;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x20 || code > 0x7e) return false;
    if (value[index] === '"' || value[index] === '\'' || value[index] === ';' || value[index] === '=' || value[index] === '\\') return false;
  }
  return true;
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, Math.round(value)));
}

function getDeviceSettingsStatusItems(
  snapshot: DeviceSettingsSnapshot | null,
  t: ReturnType<typeof useTranslation>['t'],
): Array<{ label: string; value: string }> {
  if (!snapshot) {
    return [
      {
        label: t('settings.device.statusSourceLabel', '来源'),
        value: t('settings.device.configLoading', '正在读取设备设置...'),
      },
      {
        label: t('settings.device.statusPowerLabel', '供电'),
        value: '—',
      },
      {
        label: t('settings.device.statusLowPowerLabel', '低功耗'),
        value: '—',
      },
      {
        label: t('settings.device.statusWriteLabel', '写入'),
        value: '—',
      },
    ];
  }
  const power = snapshot.activePowerSource === 'plugged'
    ? t('settings.device.powerPlugged', '插电')
    : snapshot.activePowerSource === 'battery'
      ? t('settings.device.powerBattery', '电池')
      : t('settings.device.powerUnknown', '供电未知');
  const activeLowPower = snapshot.activePowerSource === 'plugged'
    ? snapshot.pluggedLowPowerIdleMinutes
    : snapshot.activePowerSource === 'battery'
      ? snapshot.batteryLowPowerIdleMinutes
      : snapshot.lowPowerIdleMinutes;
  const lowPowerDisabled =
    activeLowPower <= 0 || (snapshot.activePowerSource === 'plugged' && !snapshot.pluggedLowPowerEnabled);
  const lowPower = lowPowerDisabled
    ? t('settings.device.lowPowerOff', '关闭')
    : t('settings.device.lowPowerCompact', '{{value}} 分钟', { value: activeLowPower });
  const source = snapshot.source === 'mock'
    ? t('settings.device.sourceMock', '浏览器预览模拟')
    : snapshot.source === 'defaults'
      ? t('settings.device.sourceDefaults', '默认值')
      : snapshot.source === 'lastKnown'
        ? t('settings.device.sourceLastKnown', '上次已知值')
        : snapshot.source === 'unavailable'
          ? t('settings.device.sourceUnavailable', '设备不可用')
          : t('settings.device.sourceFirmware', '固件');
  return [
    {
      label: t('settings.device.statusSourceLabel', '来源'),
      value: source,
    },
    {
      label: t('settings.device.statusPowerLabel', '供电'),
      value: power,
    },
    {
      label: t('settings.device.statusLowPowerLabel', '低功耗'),
      value: lowPower,
    },
    {
      label: t('settings.device.statusWriteLabel', '写入'),
      value: snapshot.writeSupported
        ? t('settings.device.configSourceWritable', '可写')
        : t('settings.device.configSourceReadOnly', '只读'),
    },
  ];
}

function formatDeviceSnapshotFooter(
  snapshot: DeviceSettingsSnapshot | null,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (!snapshot) {
    return '';
  }
  if (!snapshot.writeSupported) {
    return t('settings.device.readOnlyHint', '当前设备状态只读；连接到可写固件后可以写入。');
  }
  return '';
}
