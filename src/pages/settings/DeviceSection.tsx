import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Icon } from '../../components/Icon';
import { detectOS } from '../../components/WindowChrome';
import type { DeviceFirmwareSettingsStatus, DeviceKnobRotationAction, UserPreferences } from '../../lib/types';
import { refreshDeviceSettingsStatus } from '../../lib/ipc';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { Card } from '../_atoms';
import { FirmwareOtaPanel } from './FirmwareOtaPanel';
import { DeviceKeysPanel } from './ShortcutsSection';
import { inputStyle, SettingRow } from './shared';

const clampNumber = (value: string, fallback: number, min: number, max: number) => {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(max, Math.max(min, Math.round(parsed)));
};

const bleNameIsValid = (name: string) =>
  name.length >= 1 &&
  name.length <= 32 &&
  /^[\x21-\x7e]+$/.test(name) &&
  !/["';=\\]/.test(name);

export function DeviceSection() {
  const { t } = useTranslation();
  const { prefs, error, updatePrefs: savePrefs } = useHotkeySettings();
  const [pluggedBrightness, setPluggedBrightness] = useState('100');
  const [batteryBrightness, setBatteryBrightness] = useState('100');
  const [autoShutdownMinutes, setAutoShutdownMinutes] = useState('30');
  const [bleName, setBleName] = useState('listener');
  const [bleNameError, setBleNameError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [refreshStatus, setRefreshStatus] = useState<string | null>(null);
  const [deviceStatus, setDeviceStatus] = useState<DeviceFirmwareSettingsStatus | null>(null);

  useEffect(() => {
    if (!prefs) return;
    setPluggedBrightness(String(prefs.devicePluggedBrightnessPercent ?? 100));
    setBatteryBrightness(String(prefs.deviceBatteryBrightnessPercent ?? 100));
    setAutoShutdownMinutes(String(prefs.deviceBatteryAutoShutdownMinutes ?? 30));
    setBleName(prefs.deviceBleName || 'listener');
  }, [
    prefs?.devicePluggedBrightnessPercent,
    prefs?.deviceBatteryBrightnessPercent,
    prefs?.deviceBatteryAutoShutdownMinutes,
    prefs?.deviceBleName,
  ]);

  if (!prefs) {
    return (
      <Card>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
      </Card>
    );
  }

  const commitNumber = async (
    draft: string,
    fallback: number,
    min: number,
    max: number,
    field: keyof Pick<
      UserPreferences,
      | 'devicePluggedBrightnessPercent'
      | 'deviceBatteryBrightnessPercent'
      | 'deviceBatteryAutoShutdownMinutes'
    >,
    setDraft: (value: string) => void,
  ) => {
    const next = clampNumber(draft, fallback, min, max);
    setDraft(String(next));
    if (prefs[field] === next) return;
    await savePrefs(current => ({
      ...current,
      [field]: next,
    }));
  };

  const commitBleName = async () => {
    const next = bleName.trim();
    if (!bleNameIsValid(next)) {
      setBleNameError(
        t(
          'settings.device.bleNameError',
          '只能使用 1-32 个可见 ASCII 字符，不能包含空格、引号、分号、等号或反斜杠。',
        ),
      );
      return;
    }
    setBleNameError(null);
    if (next === prefs.deviceBleName) return;
    await savePrefs(current => ({
      ...current,
      deviceBleName: next,
    }));
  };

  const refreshFromDevice = async () => {
    if (refreshing) return;
    setRefreshing(true);
    setRefreshError(null);
    setRefreshStatus(null);
    try {
      const status = await refreshDeviceSettingsStatus();
      setDeviceStatus(status);
      const knobRotationAction = firmwareKnobActionToUi(status.knobRotationAction);
      await savePrefs(current => ({
        ...current,
        devicePluggedBrightnessPercent: status.pluggedBrightnessPercent,
        deviceBatteryBrightnessPercent: status.batteryBrightnessPercent,
        deviceBatteryAutoShutdownMinutes: status.batteryAutoShutdownMinutes,
        deviceBleName: status.bleName,
        deviceKnobRotationAction: knobRotationAction,
      }));
      setRefreshStatus(
        t(
          'settings.device.refreshOk',
          '已读取设备当前设置',
        ),
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setRefreshError(message);
    } finally {
      setRefreshing(false);
    }
  };
  const bleSupported = detectOS() === 'win';

  return (
    <>
    <Card>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, marginBottom: 4 }}>
        <div style={{ fontSize: 13, fontWeight: 600 }}>
          {t('settings.device.title', '设备')}
        </div>
        <button
          type="button"
          onClick={() => void refreshFromDevice()}
          disabled={refreshing}
          title={t('settings.device.refresh', '刷新设备状态')}
          aria-label={t('settings.device.refresh', '刷新设备状态')}
          style={{
            width: 28,
            height: 28,
            borderRadius: 8,
            border: '0.5px solid var(--ol-line-strong)',
            background: refreshing ? 'var(--ol-control-track)' : 'var(--ol-surface-2)',
            color: refreshing ? 'var(--ol-ink-4)' : 'var(--ol-ink)',
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            cursor: 'default',
            opacity: refreshing ? 0.75 : 1,
          }}
        >
          <Icon name="refresh" size={15} />
        </button>
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, marginBottom: 2 }}>
        {t('settings.device.desc', '设备亮度、低功耗和蓝牙名称会同步写入已连接的 Listener。')}
      </div>
      {error && (
        <div
          style={{
            marginTop: 10,
            padding: '8px 10px',
            borderRadius: 8,
            background: 'rgba(189, 98, 89, 0.12)',
            color: 'var(--ol-err)',
            fontSize: 11.5,
            lineHeight: 1.45,
          }}
        >
          {error}
        </div>
      )}
      {refreshError && (
        <div
          style={{
            marginTop: 10,
            padding: '8px 10px',
            borderRadius: 8,
            background: 'rgba(189, 98, 89, 0.12)',
            color: 'var(--ol-err)',
            fontSize: 11.5,
            lineHeight: 1.45,
          }}
        >
          {refreshError}
        </div>
      )}
      {refreshStatus && !refreshError && (
        <div style={{ marginTop: 8, fontSize: 11.5, color: 'var(--ol-ink-4)' }}>
          {refreshStatus}
        </div>
      )}
      <SettingRow
        label={t('settings.device.pluggedBrightnessLabel', '插电亮度')}
        desc={t('settings.device.pluggedBrightnessDesc', 'USB/充电时的灯效亮度上限。')}
      >
        <PercentInput
          value={pluggedBrightness}
          onChange={setPluggedBrightness}
          onCommit={() =>
            void commitNumber(
              pluggedBrightness,
              prefs.devicePluggedBrightnessPercent ?? 100,
              0,
              100,
              'devicePluggedBrightnessPercent',
              setPluggedBrightness,
            )
          }
        />
      </SettingRow>
      <SettingRow
        label={t('settings.device.batteryBrightnessLabel', '电池亮度')}
        desc={t('settings.device.batteryBrightnessDesc', '拔电后使用的灯效亮度上限。')}
      >
        <PercentInput
          value={batteryBrightness}
          onChange={setBatteryBrightness}
          onCommit={() =>
            void commitNumber(
              batteryBrightness,
              prefs.deviceBatteryBrightnessPercent ?? 100,
              0,
              100,
              'deviceBatteryBrightnessPercent',
              setBatteryBrightness,
            )
          }
        />
      </SettingRow>
      <SettingRow
        label={t('settings.device.autoShutdownLabel', '电池自动关机')}
        desc={t('settings.device.autoShutdownDesc', '仅电池供电时生效，插电时保持唤醒。')}
      >
        <NumberInput
          value={autoShutdownMinutes}
          suffix={t('settings.device.minuteSuffix', '分钟')}
          min={1}
          max={1440}
          onChange={setAutoShutdownMinutes}
          onCommit={() =>
            void commitNumber(
              autoShutdownMinutes,
              prefs.deviceBatteryAutoShutdownMinutes ?? 30,
              1,
              1440,
              'deviceBatteryAutoShutdownMinutes',
              setAutoShutdownMinutes,
            )
          }
        />
      </SettingRow>
      <SettingRow
        label={t('settings.device.bleNameLabel', '蓝牙名称')}
        desc={t('settings.device.bleNameDesc', '写入固件后，重新广播或重启后显示新名称。')}
      >
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, width: '100%', maxWidth: 360 }}>
          <input
            value={bleName}
            onChange={event => {
              setBleName(event.target.value);
              setBleNameError(null);
            }}
            onBlur={() => void commitBleName()}
            onKeyDown={event => {
              if (event.key === 'Enter') {
                event.currentTarget.blur();
              }
            }}
            maxLength={32}
            spellCheck={false}
            style={inputStyle}
          />
          {bleNameError && (
            <div style={{ fontSize: 11, color: 'var(--ol-err)', lineHeight: 1.45 }}>
              {bleNameError}
            </div>
          )}
          {deviceStatus && (
            <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', lineHeight: 1.45 }}>
              {deviceStatus.bleNamePendingRestart
                ? t(
                    'settings.device.bleNamePending',
                    '设备已保存新名称；系统列表可能要重启蓝牙或重新配对后更新。',
                  )
                : t('settings.device.bleNameActive', '设备当前广播名称已是最新。')}
            </div>
          )}
        </div>
      </SettingRow>
    </Card>
    <DeviceKeysPanel />
    <FirmwareOtaPanel supported={bleSupported} bleStatus="idle" />
    </>
  );
}

function firmwareKnobActionToUi(value: string): DeviceKnobRotationAction {
  switch (value) {
    case 'screen_brightness':
      return 'screenBrightness';
    case 'disabled':
      return 'disabled';
    case 'system_volume':
    default:
      return 'systemVolume';
  }
}

function PercentInput({
  value,
  onChange,
  onCommit,
}: {
  value: string;
  onChange: (value: string) => void;
  onCommit: () => void;
}) {
  return (
    <NumberInput
      value={value}
      suffix="%"
      min={0}
      max={100}
      onChange={onChange}
      onCommit={onCommit}
    />
  );
}

function NumberInput({
  value,
  suffix,
  min,
  max,
  onChange,
  onCommit,
}: {
  value: string;
  suffix: string;
  min: number;
  max: number;
  onChange: (value: string) => void;
  onCommit: () => void;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8, width: '100%', maxWidth: 180 }}>
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        onChange={event => onChange(event.target.value)}
        onBlur={onCommit}
        onKeyDown={event => {
          if (event.key === 'Enter') {
            event.currentTarget.blur();
          }
        }}
        style={{ ...inputStyle, maxWidth: 120, textAlign: 'right' }}
      />
      <span style={{ fontSize: 12, color: 'var(--ol-ink-4)', minWidth: 28 }}>{suffix}</span>
    </div>
  );
}
