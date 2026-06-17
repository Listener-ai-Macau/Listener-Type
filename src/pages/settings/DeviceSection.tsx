import { useEffect, useMemo, useState } from 'react';
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
import { Btn, Card } from '../_atoms';
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

      <FirmwareOtaPanel
        supported={bleSupported}
        bleStatus={bleStatus}
      />
    </>
  );
}

function DeviceFirmwareSettingsCard() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<DeviceSettingsSnapshot | null>(null);
  const [form, setForm] = useState<DeviceSettingsUpdateRequest>({
    pluggedBrightnessPercent: 100,
    batteryBrightnessPercent: 100,
    lowPowerIdleMinutes: 1,
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

  const save = async () => {
    if (writeDisabled) return;
    setStatus('saving');
    setMessage('');
    try {
      const value = await setDeviceSettings(form);
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

  return (
    <Card>
      <div className="ol-device-settings-header" style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', gap: 12, marginBottom: 6 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 600 }}>
            {t('settings.device.configTitle', '设备设置')}
          </div>
          <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, lineHeight: 1.5 }}>
            {t('settings.device.configDesc', '亮度按供电状态分开保存；自动关机只在拔电后的电池模式生效。')}
          </div>
        </div>
        <div className="ol-device-settings-toolbar" style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', justifyContent: 'flex-end' }}>
          <div className="ol-device-readwrite-row" style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 8, width: 'min(100%, 220px)' }}>
            <Btn variant="ghost" size="sm" icon="refresh" onClick={() => void refresh()} disabled={readDisabled} style={{ justifyContent: 'center', minWidth: 0, whiteSpace: 'nowrap' }}>
              {status === 'loading'
                ? t('settings.device.reading', '读取中')
                : t('settings.device.readFromDevice', '读取设备')}
            </Btn>
            <Btn variant="blue" size="sm" icon="check" disabled={writeDisabled} onClick={() => void save()} style={{ justifyContent: 'center', minWidth: 0, whiteSpace: 'nowrap' }}>
              {status === 'saving'
                ? t('settings.device.writing', '写入中')
                : t('settings.device.writeToDevice', '写入设备')}
            </Btn>
          </div>
        </div>
      </div>

      {detailText && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginBottom: 2 }}>
          {detailText}
        </div>
      )}

      <SettingRow
        label={t('settings.device.pluggedBrightnessLabel', '插电亮度')}
        desc={t('settings.device.pluggedBrightnessDesc', 'USB / 充电 / 外部供电时使用，不参与低功耗关机判断。')}
      >
        <PercentSlider
          value={form.pluggedBrightnessPercent}
          disabled={!snapshot?.writeSupported || status === 'saving'}
          onChange={value => setForm(current => ({ ...current, pluggedBrightnessPercent: value }))}
        />
      </SettingRow>
      <SettingRow
        label={t('settings.device.batteryBrightnessLabel', '电池亮度')}
        desc={t('settings.device.batteryBrightnessDesc', '拔电后使用，适合把状态灯、按键灯、旋钮灯和板框灯整体压低。')}
      >
        <PercentSlider
          value={form.batteryBrightnessPercent}
          disabled={!snapshot?.writeSupported || status === 'saving'}
          onChange={value => setForm(current => ({ ...current, batteryBrightnessPercent: value }))}
        />
      </SettingRow>
      <SettingRow
        label={t('settings.device.lowPowerIdleLabel', '低功耗等待')}
        desc={t('settings.device.lowPowerIdleDesc', '无操作多久后进入低功耗空闲；范围 1-1440 分钟。')}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, width: '100%', flexWrap: 'wrap' }}>
          <input
            type="number"
            min={1}
            max={1440}
            value={form.lowPowerIdleMinutes}
            disabled={!snapshot?.writeSupported || status === 'saving'}
            onChange={event => {
              const value = Number(event.target.value);
              setForm(current => ({ ...current, lowPowerIdleMinutes: Number.isFinite(value) ? value : current.lowPowerIdleMinutes }));
            }}
            style={{ ...inputStyle, flex: '0 1 96px', maxWidth: 120 }}
          />
          <span style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>
            {t('settings.device.minutes', '分钟')}
          </span>
        </div>
      </SettingRow>
      <SettingRow
        label={t('settings.device.autoShutdownLabel', '电池自动关机')}
        desc={t('settings.device.autoShutdownDesc', '仅电池供电且长时间空闲时生效；插电、充电或外部供电会阻止自动关机。')}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, width: '100%', flexWrap: 'wrap' }}>
          <input
            type="number"
            min={1}
            max={1440}
            value={form.batteryAutoShutdownMinutes}
            disabled={!snapshot?.writeSupported || status === 'saving'}
            onChange={event => {
              const value = Number(event.target.value);
              setForm(current => ({ ...current, batteryAutoShutdownMinutes: Number.isFinite(value) ? value : current.batteryAutoShutdownMinutes }));
            }}
            style={{ ...inputStyle, flex: '0 1 96px', maxWidth: 120 }}
          />
          <span style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>
            {t('settings.device.minutes', '分钟')}
          </span>
        </div>
      </SettingRow>
      <SettingRow
        label={t('settings.device.bleNameLabel', '蓝牙名称')}
        desc={t('settings.device.bleNameDesc', '1-32 个 ASCII 字符；部分 Windows 设备名变更需要重连或重新配对后才显示。')}
      >
        <input
          value={form.bleName}
          maxLength={32}
          disabled={!snapshot?.writeSupported || status === 'saving'}
          onChange={event => setForm(current => ({ ...current, bleName: event.target.value }))}
          style={{ ...inputStyle, maxWidth: 'none' }}
        />
      </SettingRow>

      <div style={{ fontSize: 11.5, color: validationError || status === 'error' ? 'var(--ol-err)' : status === 'saved' ? 'var(--ol-ok)' : 'var(--ol-ink-4)', lineHeight: 1.45, paddingTop: 12, borderTop: '0.5px solid var(--ol-line-soft)' }}>
        {validationError || message || formatDeviceSnapshotSummary(snapshot, t)}
      </div>
    </Card>
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
    <div style={{ display: 'flex', alignItems: 'center', gap: 8, width: '100%', flexWrap: 'wrap' }}>
      <input
        type="range"
        min={0}
        max={100}
        value={safeValue}
        disabled={disabled}
        onChange={event => onChange(clampPercent(Number(event.target.value)))}
        style={{ flex: '1 1 88px', minWidth: 72, accentColor: 'var(--ol-blue)' }}
      />
      <input
        type="number"
        min={0}
        max={100}
        value={safeValue}
        disabled={disabled}
        onChange={event => onChange(clampPercent(Number(event.target.value)))}
        style={{ ...inputStyle, flex: '0 1 68px', maxWidth: 76, textAlign: 'right' }}
      />
      <span style={{ fontSize: 12, color: 'var(--ol-ink-4)', width: 16 }}>%</span>
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
            gridTemplateColumns: 'minmax(92px, 140px) minmax(0, 1fr)',
            gap: 12,
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
            gridTemplateColumns: 'minmax(92px, 140px) minmax(0, 1fr)',
            gap: 12,
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
              gridTemplateColumns: 'minmax(92px, 140px) minmax(0, 1fr)',
              gap: 12,
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
              gridTemplateColumns: '50px minmax(0, 1fr)',
              columnGap: 16,
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
    pluggedBrightnessPercent: snapshot.pluggedBrightnessPercent,
    batteryBrightnessPercent: snapshot.batteryBrightnessPercent,
    lowPowerIdleMinutes: snapshot.lowPowerIdleMinutes,
    batteryAutoShutdownMinutes: Math.max(1, Math.round(snapshot.batteryAutoShutdownMs / 60000)),
    bleName: snapshot.bleName,
  };
}

function validateDeviceSettingsForm(
  form: DeviceSettingsUpdateRequest,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (!Number.isFinite(form.pluggedBrightnessPercent) || form.pluggedBrightnessPercent < 0 || form.pluggedBrightnessPercent > 100) {
    return t('settings.device.errorBrightness', '亮度必须在 0-100 之间。');
  }
  if (!Number.isFinite(form.batteryBrightnessPercent) || form.batteryBrightnessPercent < 0 || form.batteryBrightnessPercent > 100) {
    return t('settings.device.errorBrightness', '亮度必须在 0-100 之间。');
  }
  if (!Number.isFinite(form.lowPowerIdleMinutes) || form.lowPowerIdleMinutes < 1 || form.lowPowerIdleMinutes > 1440) {
    return t('settings.device.errorLowPowerIdle', '低功耗等待时间必须在 1-1440 分钟之间。');
  }
  if (!Number.isFinite(form.batteryAutoShutdownMinutes) || form.batteryAutoShutdownMinutes < 1 || form.batteryAutoShutdownMinutes > 1440) {
    return t('settings.device.errorAutoShutdown', '自动关机时间必须在 1-1440 分钟之间。');
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

function formatDeviceSnapshotSummary(
  snapshot: DeviceSettingsSnapshot | null,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (!snapshot) {
    return t('settings.device.configLoading', '正在读取设备设置...');
  }
  const power = snapshot.activePowerSource === 'plugged'
    ? t('settings.device.powerPlugged', '插电')
    : snapshot.activePowerSource === 'battery'
      ? t('settings.device.powerBattery', '电池')
      : t('settings.device.powerUnknown', '供电未知');
  const active = snapshot.activeBrightnessPercent == null
    ? ''
    : t('settings.device.activeBrightness', '当前上限 {{value}}%', { value: snapshot.activeBrightnessPercent });
  const lowPower = t('settings.device.lowPowerSummary', '低功耗 {{value}} 分钟', { value: snapshot.lowPowerIdleMinutes });
  const source = snapshot.source === 'mock'
    ? t('settings.device.sourceMock', '浏览器预览模拟')
    : snapshot.source === 'defaults'
      ? t('settings.device.sourceDefaults', '默认值')
      : snapshot.source === 'lastKnown'
        ? t('settings.device.sourceLastKnown', '上次已知值')
        : snapshot.source === 'unavailable'
          ? t('settings.device.sourceUnavailable', '设备不可用')
          : t('settings.device.sourceFirmware', '固件');
  return [source, power, active, lowPower].filter(Boolean).join(' · ');
}
