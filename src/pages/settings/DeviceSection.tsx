import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { DeviceKnobRotationAction, UserPreferences } from '../../lib/types';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { Card } from '../_atoms';
import { inputStyle, SettingRow } from './shared';

const KNOB_ROTATION_ACTIONS: DeviceKnobRotationAction[] = [
  'systemVolume',
  'screenBrightness',
  'disabled',
];

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

  const setKnobRotationAction = async (action: DeviceKnobRotationAction) => {
    if (action === prefs.deviceKnobRotationAction) return;
    await savePrefs(current => ({
      ...current,
      deviceKnobRotationAction: action,
    }));
  };

  return (
    <Card>
      <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>
        {t('settings.device.title', '设备')}
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, marginBottom: 2 }}>
        {t('settings.device.desc', '设备亮度、低功耗、蓝牙名称和旋钮动作会同步写入已连接的 Listener。')}
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
        </div>
      </SettingRow>
      <SettingRow
        label={t('settings.device.knobRotationLabel', '旋钮旋转')}
        desc={t('settings.device.knobRotationDesc', '旋转动作同步到设备；按压类手势仍由固件固定。')}
      >
        <div
          role="group"
          aria-label={t('settings.device.knobRotationAria', '选择旋钮旋转动作')}
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(3, minmax(0, 1fr))',
            width: '100%',
            maxWidth: 360,
            padding: 3,
            borderRadius: 10,
            background: 'var(--ol-control-track)',
            border: '0.5px solid var(--ol-line-strong)',
            gap: 2,
          }}
        >
          {KNOB_ROTATION_ACTIONS.map(action => {
            const active = prefs.deviceKnobRotationAction === action;
            return (
              <button
                key={action}
                type="button"
                aria-pressed={active}
                onClick={() => void setKnobRotationAction(action)}
                style={{
                  minHeight: 32,
                  padding: '0 10px',
                  border: '0.5px solid',
                  borderColor: active ? 'var(--ol-blue)' : 'transparent',
                  borderRadius: 8,
                  background: active ? 'var(--ol-control-active)' : 'transparent',
                  color: active ? 'var(--ol-ink)' : 'var(--ol-ink-3)',
                  fontFamily: 'inherit',
                  fontSize: 12,
                  fontWeight: active ? 600 : 500,
                  cursor: 'default',
                }}
              >
                {t(`settings.device.knobActions.${action}`, knobActionFallback(action))}
              </button>
            );
          })}
        </div>
      </SettingRow>
    </Card>
  );
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

function knobActionFallback(action: DeviceKnobRotationAction) {
  switch (action) {
    case 'screenBrightness':
      return '屏幕亮度';
    case 'disabled':
      return '禁用';
    case 'systemVolume':
    default:
      return '电脑音量';
  }
}
