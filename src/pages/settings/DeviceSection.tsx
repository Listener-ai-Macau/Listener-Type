import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ShortcutRecorder } from '../../components/ShortcutRecorder';
import { detectOS } from '../../components/WindowChrome';
import { SelectLite } from '../../components/ui/SelectLite';
import { defaultAppShortcutModifiers, formatComboLabel } from '../../lib/hotkey';
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
import { Btn, Card, Pill, type PillTone } from '../_atoms';
import { FirmwareOtaPanel } from './FirmwareOtaPanel';
import { inputStyle, SettingRow } from './shared';
import type { EmbeddedBleProbeStatus } from '../../components/EmbeddedBleStatusPanel';

const DEVICE_KEYS: Array<{ id: DeviceCustomKeyId }> = [
  { id: 'key1' },
  { id: 'key2' },
  { id: 'key3' },
  { id: 'key4' },
];

type DeviceKeyMapKey =
  | 'deviceCustomKeys'
  | 'deviceCustomKeyDoubleClicks'
  | 'deviceCustomKeyLongPresses';

const DEVICE_GESTURES: Array<{
  id: DeviceCustomKeyGesture;
  mapKey: DeviceKeyMapKey;
  fallbacks: Record<DeviceCustomKeyId, string>;
}> = [
  {
    id: 'singleClick',
    mapKey: 'deviceCustomKeys',
    fallbacks: { key1: 'F13', key2: 'F14', key3: 'F15', key4: 'F16' },
  },
  {
    id: 'doubleClick',
    mapKey: 'deviceCustomKeyDoubleClicks',
    fallbacks: { key1: 'F17', key2: 'F18', key3: 'F19', key4: 'F20' },
  },
  {
    id: 'longPress',
    mapKey: 'deviceCustomKeyLongPresses',
    fallbacks: { key1: 'F21', key2: 'F22', key3: 'F23', key4: 'F24' },
  },
];

const DEVICE_KEY_ACTIONS: DeviceCustomKeyAction[] = [
  'disabled',
  'openApp',
  'openExternalApp',
  'switchStyle',
  'translation',
  'selectionAsk',
  'pasteTemplate',
  'sendShortcut',
  'dictation',
  'copyShortcut',
  'pasteShortcut',
  'undoShortcut',
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

const KNOB_ACTION_ROWS = [
  { gesture: 'rotate', action: 'systemVolume', locked: false },
  { gesture: 'shortPress', action: 'recording', locked: true },
  { gesture: 'doubleClick', action: 'bluetoothReset', locked: true },
  { gesture: 'longPress', action: 'powerOff', locked: true },
] as const;

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
          knobRotationAction={knobRotationAction}
          onKnobRotationActionChange={updateKnobRotationAction}
        />
      </Card>
    </>
  );
}

function DeviceFirmwareSettingsCard() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<DeviceSettingsSnapshot | null>(null);
  const [form, setForm] = useState<DeviceSettingsUpdateRequest>({
    pluggedBrightnessPercent: 100,
    batteryBrightnessPercent: 100,
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
  const sourceTone: PillTone = snapshot?.connected ? (snapshot.writeSupported ? 'ok' : 'blue') : 'outline';
  const sourceLabel = snapshot
    ? snapshot.writeSupported
      ? t('settings.device.configSourceWritable', '可写')
      : t('settings.device.configSourceReadOnly', '只读')
    : t('common.loading');

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

  return (
    <Card>
      <div style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', gap: 12, marginBottom: 6 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 600 }}>
            {t('settings.device.configTitle', '设备设置')}
          </div>
          <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, lineHeight: 1.5 }}>
            {t('settings.device.configDesc', '亮度按供电状态分开保存；自动关机只在拔电后的电池模式生效。')}
          </div>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', justifyContent: 'flex-end' }}>
          <Pill tone={sourceTone} size="sm">{sourceLabel}</Pill>
          <Btn variant="ghost" size="sm" icon="refresh" onClick={() => void refresh()} disabled={status === 'loading' || status === 'saving'} style={{ whiteSpace: 'nowrap' }}>
            {status === 'loading' ? t('common.loading') : t('common.refresh')}
          </Btn>
        </div>
      </div>

      {snapshot?.detail && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginBottom: 2 }}>
          {snapshot.detail}
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

      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12, paddingTop: 12, borderTop: '0.5px solid var(--ol-line-soft)', flexWrap: 'wrap' }}>
        <div style={{ fontSize: 11.5, color: validationError || status === 'error' ? 'var(--ol-err)' : status === 'saved' ? 'var(--ol-ok)' : 'var(--ol-ink-4)', lineHeight: 1.45, minWidth: 0, flex: '1 1 220px' }}>
          {validationError || message || formatDeviceSnapshotSummary(snapshot, t)}
        </div>
        <Btn variant="blue" size="sm" icon="check" disabled={writeDisabled} onClick={() => void save()} style={{ whiteSpace: 'nowrap' }}>
          {status === 'saving'
            ? t('common.saving', '保存中')
            : t('common.save', '保存')}
        </Btn>
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
  knobRotationAction,
  onKnobRotationActionChange,
}: {
  knobRotationAction: DeviceKnobRotationAction;
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
        {KNOB_ACTION_ROWS.map(item => (
          <div
            key={item.gesture}
            style={{
              display: 'grid',
              gridTemplateColumns: 'minmax(92px, 140px) minmax(0, 1fr)',
              gap: 12,
              alignItems: 'center',
              opacity: item.locked ? 0.66 : 1,
            }}
          >
            <div style={{ fontSize: 12.5, fontWeight: 600, color: item.locked ? 'var(--ol-ink-3)' : 'var(--ol-ink)' }}>
              {t(`settings.deviceKeys.knob.gestures.${item.gesture}`)}
            </div>
            {item.gesture === 'rotate' ? (
              <SelectLite
                value={knobRotationAction}
                onChange={value => void onKnobRotationActionChange(value)}
                options={KNOB_ROTATION_ACTIONS.map(action => ({
                  value: action,
                  label: t(`settings.deviceKeys.knob.actions.${action}`),
                }))}
                style={{ ...inputStyle, maxWidth: 'none', minWidth: 0 }}
                ariaLabel={t('settings.deviceKeys.knob.rotationActionSelectAria')}
              />
            ) : (
              <div
                aria-disabled
                style={{
                  minHeight: 32,
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 8,
                  width: '100%',
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
            )}
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
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        {DEVICE_KEYS.map(({ id }) => (
          <div
            key={`${gesture.id}-${id}`}
            style={{
              display: 'grid',
              gridTemplateColumns: 'minmax(92px, 140px) minmax(0, 1fr)',
              gap: 12,
              alignItems: 'start',
            }}
          >
            <div style={{ minWidth: 0, paddingTop: 6 }}>
              <div style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)' }}>
                {t('settings.deviceKeys.keyLabel', { key: id.toUpperCase() })}
              </div>
              <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 2 }}>
                {t('settings.deviceKeys.fallback', { fallback: gesture.fallbacks[id] })}
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
  onChange,
}: {
  mapping: DeviceCustomKeyMapping;
  installedApps: InstalledApplication[];
  installedAppsLoading: boolean;
  autoOpen?: boolean;
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
    <div style={{ display: 'grid', gridTemplateColumns: 'minmax(130px, 165px) minmax(0, 1fr)', gap: 8, width: '100%', alignItems: 'start' }}>
      <SelectLite
        value={mapping.action}
        onChange={value => void updateAction(value as DeviceCustomKeyAction)}
        options={DEVICE_KEY_ACTIONS.map(action => ({
          value: action,
          label: t(`settings.deviceKeys.actions.${action}`),
        }))}
        defaultOpen={autoOpen}
        style={{ ...inputStyle, maxWidth: 'none', minWidth: 0 }}
        ariaLabel={t('settings.deviceKeys.actionSelectAria')}
      />
      <div style={{ minWidth: 0 }}>
        {mapping.action === 'openApp' && (
          <SelectLite
            value={mapping.appPage ?? 'settingsDevice'}
            onChange={value => void onChange({ ...mapping, appPage: value as DeviceCustomKeyAppPage })}
            options={DEVICE_KEY_APP_PAGES.map(page => ({
              value: page,
              label: t(`settings.deviceKeys.appPages.${page}`),
            }))}
            style={{ ...inputStyle, maxWidth: 'none', minWidth: 0 }}
            ariaLabel={t('settings.deviceKeys.appPageSelectAria')}
          />
        )}
        {mapping.action === 'openExternalApp' && (
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(130px, 1fr))', gap: 8, width: '100%' }}>
            <SelectLite
              value={externalAppPickerValue}
              onChange={value => {
                if (value === EXTERNAL_APP_MANUAL_VALUE) return;
                void onChange({ ...mapping, externalAppPath: value });
              }}
              options={externalAppOptions}
              style={{ ...inputStyle, maxWidth: 'none', minWidth: 0 }}
              ariaLabel={t('settings.deviceKeys.installedAppSelectAria')}
            />
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
              style={{ ...inputStyle, maxWidth: 'none' }}
            />
          </div>
        )}
        {mapping.action === 'openExternalApp' && externalAppPath && !matchingInstalledApp && (
          <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', marginTop: 4 }}>
            {t('settings.deviceKeys.installedAppManualHint')}
          </div>
        )}
        {mapping.action === 'pasteTemplate' && (
          <input
            value={mapping.pasteTemplate}
            onChange={event => {
              const pasteTemplate = event.target.value;
              void onChange({ ...mapping, pasteTemplate });
            }}
            placeholder={t('settings.deviceKeys.templatePlaceholder')}
            style={{ ...inputStyle, maxWidth: 'none' }}
          />
        )}
        {mapping.action === 'sendShortcut' && (
          <ShortcutRecorder
            value={mapping.shortcut ?? fallbackShortcut()}
            alignRecordButton
            onSave={async shortcut => {
              await onChange({ ...mapping, shortcut });
            }}
          />
        )}
        {mapping.action !== 'pasteTemplate' &&
          mapping.action !== 'sendShortcut' &&
          mapping.action !== 'openApp' &&
          mapping.action !== 'openExternalApp' && (
          <span style={{ display: 'inline-flex', minHeight: 32, alignItems: 'center', padding: '0 10px', borderRadius: 6, background: 'var(--ol-surface-2)', border: '0.5px solid var(--ol-line-strong)', fontSize: 12, color: 'var(--ol-ink-3)' }}>
            {mapping.action === 'disabled'
              ? t('settings.deviceKeys.noop')
              : t('settings.deviceKeys.actionReady')}
          </span>
        )}
        {mapping.action === 'sendShortcut' && mapping.shortcut && (
          <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', marginTop: 4 }}>
            {formatComboLabel(mapping.shortcut)}
          </div>
        )}
      </div>
    </div>
  );
}

function snapshotToForm(snapshot: DeviceSettingsSnapshot): DeviceSettingsUpdateRequest {
  return {
    pluggedBrightnessPercent: snapshot.pluggedBrightnessPercent,
    batteryBrightnessPercent: snapshot.batteryBrightnessPercent,
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
  if (!Number.isFinite(form.batteryAutoShutdownMinutes) || form.batteryAutoShutdownMinutes < 1 || form.batteryAutoShutdownMinutes > 1440) {
    return t('settings.device.errorAutoShutdown', '自动关机时间必须在 1-1440 分钟之间。');
  }
  if (!isValidBleName(form.bleName)) {
    return t('settings.device.errorBleName', '蓝牙名称必须是 1-32 个可打印 ASCII 字符，不能包含空格、引号、分号、等号或反斜杠。');
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
  const source = snapshot.source === 'mock'
    ? t('settings.device.sourceMock', '浏览器预览模拟')
    : snapshot.source === 'defaults'
      ? t('settings.device.sourceDefaults', '默认值')
      : snapshot.source === 'lastKnown'
        ? t('settings.device.sourceLastKnown', '上次已知值')
        : snapshot.source === 'unavailable'
          ? t('settings.device.sourceUnavailable', '设备不可用')
          : t('settings.device.sourceFirmware', '固件');
  return [source, power, active].filter(Boolean).join(' · ');
}
