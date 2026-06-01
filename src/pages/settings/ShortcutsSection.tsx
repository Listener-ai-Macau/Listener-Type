// 快捷键设置：开始/停止、翻译、问答、切风格、唤起 App、以及只读取消/确认提示。

import { useTranslation } from 'react-i18next';
import { ShortcutRecorder } from '../../components/ShortcutRecorder';
import { defaultAppShortcutModifiers, defaultQaShortcut, formatComboLabel } from '../../lib/hotkey';
import {
  setDictationHotkey,
  setOpenAppHotkey,
  setQaHotkey,
  setSwitchStyleHotkey,
  setTranslationHotkey,
} from '../../lib/ipc';
import type {
  DeviceCustomKeyAction,
  DeviceCustomKeyId,
  DeviceCustomKeyMapping,
  ShortcutBinding,
} from '../../lib/types';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { Card } from '../_atoms';
import { inputStyle, SettingRow } from './shared';

const DEVICE_KEYS: Array<{ id: DeviceCustomKeyId; fallback: string }> = [
  { id: 'key1', fallback: 'F13' },
  { id: 'key2', fallback: 'F14' },
  { id: 'key3', fallback: 'F15' },
  { id: 'key4', fallback: 'F16' },
];

const DEVICE_KEY_ACTIONS: DeviceCustomKeyAction[] = [
  'disabled',
  'openApp',
  'switchStyle',
  'translation',
  'selectionAsk',
  'pasteTemplate',
  'sendShortcut',
];

const fallbackShortcut = (): ShortcutBinding => ({
  primary: 'K',
  modifiers: defaultAppShortcutModifiers(),
});

export function ShortcutsSection() {
  const { t } = useTranslation();
  const { prefs, hotkey, capability, updatePrefs: savePrefs } = useHotkeySettings();

  if (!prefs || !hotkey || !capability) {
    return (
      <Card>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
      </Card>
    );
  }

  const desc = capability.requiresAccessibilityPermission
    ? t('settings.shortcuts.descAcc')
    : t('settings.shortcuts.descNoAcc');
  const readonlyRows: Array<[string, string]> = [
    [t('settings.shortcuts.cancel'), 'Esc'],
    [t('settings.shortcuts.confirm'), t('settings.shortcuts.confirmHint')],
  ];
  return (
    <Card>
      <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>{t('settings.shortcuts.title')}</div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginBottom: 6 }}>{desc}</div>
      <SettingRow label={t('settings.shortcuts.startStop')}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, width: '100%' }}>
          <ShortcutRecorder
            value={prefs.dictationHotkey}
            alignRecordButton
            onSave={async binding => {
              await setDictationHotkey(binding);
              await savePrefs({ ...prefs, dictationHotkey: binding });
            }}
          />
          <div style={{ fontSize: 11, color: 'var(--ol-ink-4)' }}>
            {t('hotkey.modeToggleSuffix')}
          </div>
        </div>
      </SettingRow>
      <SettingRow label={t('translation.hotkey.title', 'Translation shortcut')}>
        <ShortcutRecorder
          value={prefs.translationHotkey}
          alignRecordButton
          onSave={async binding => {
            await setTranslationHotkey(binding);
            await savePrefs({ ...prefs, translationHotkey: binding });
          }}
        />
      </SettingRow>
      <SettingRow label={t('selectionAsk.hotkey.title')}>
        {prefs.qaHotkey ? (
          <ShortcutRecorder
            value={prefs.qaHotkey}
            alignRecordButton
            onSave={async binding => {
              await setQaHotkey(binding);
              await savePrefs({ ...prefs, qaHotkey: binding });
            }}
          />
        ) : (
          <button
            onClick={async () => {
              const binding = defaultQaShortcut();
              await setQaHotkey(binding);
              await savePrefs({ ...prefs, qaHotkey: binding });
            }}
            style={{ fontSize: 12, padding: '5px 14px', background: 'var(--ol-blue)', color: '#fff', border: 0, borderRadius: 6, fontFamily: 'inherit', fontWeight: 500, cursor: 'default' }}
          >
            {t('selectionAsk.hotkey.enable', 'Enable')}
          </button>
        )}
      </SettingRow>
      <SettingRow label={t('settings.shortcuts.switchStyle')}>
        <ShortcutRecorder
          value={prefs.switchStyleHotkey}
          alignRecordButton
          onSave={async binding => {
            await setSwitchStyleHotkey(binding);
            await savePrefs({ ...prefs, switchStyleHotkey: binding });
          }}
        />
      </SettingRow>
      <SettingRow label={t('settings.shortcuts.openApp')}>
        <ShortcutRecorder
          value={prefs.openAppHotkey}
          alignRecordButton
          onSave={async binding => {
            await setOpenAppHotkey(binding);
            await savePrefs({ ...prefs, openAppHotkey: binding });
          }}
        />
      </SettingRow>
      <div style={{ fontSize: 13, fontWeight: 600, marginTop: 10, paddingTop: 14, borderTop: '0.5px solid var(--ol-line-soft)' }}>
        {t('settings.deviceKeys.title')}
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, marginBottom: 2 }}>
        {t('settings.deviceKeys.desc')}
      </div>
      {DEVICE_KEYS.map(({ id, fallback }) => (
        <SettingRow
          key={id}
          label={t('settings.deviceKeys.keyLabel', { key: id.toUpperCase() })}
          desc={t('settings.deviceKeys.fallback', { fallback })}
        >
          <DeviceKeyMappingControl
            mapping={prefs.deviceCustomKeys[id]}
            onChange={async mapping => {
              await savePrefs(current => ({
                ...current,
                deviceCustomKeys: {
                  ...current.deviceCustomKeys,
                  [id]: mapping,
                },
              }));
            }}
          />
        </SettingRow>
      ))}
      {readonlyRows.map(([k, v]) => (
        <SettingRow key={k} label={k}>
          <kbd style={{
            display: 'inline-flex', alignItems: 'center', gap: 4,
            padding: '4px 10px', fontSize: 12, fontFamily: 'var(--ol-font-mono)',
            borderRadius: 6, background: 'var(--ol-surface-2)',
            border: '0.5px solid var(--ol-line-strong)',
            boxShadow: '0 1px 0 rgba(0,0,0,0.04)',
            color: 'var(--ol-ink-2)',
          }}>{v}</kbd>
        </SettingRow>
      ))}
    </Card>
  );
}

function DeviceKeyMappingControl({
  mapping,
  onChange,
}: {
  mapping: DeviceCustomKeyMapping;
  onChange: (mapping: DeviceCustomKeyMapping) => Promise<void>;
}) {
  const { t } = useTranslation();
  const updateAction = async (action: DeviceCustomKeyAction) => {
    await onChange({
      ...mapping,
      action,
      shortcut: action === 'sendShortcut' ? mapping.shortcut ?? fallbackShortcut() : mapping.shortcut,
    });
  };

  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'minmax(130px, 165px) minmax(0, 1fr)', gap: 8, width: '100%', alignItems: 'start' }}>
      <select
        value={mapping.action}
        onChange={event => void updateAction(event.target.value as DeviceCustomKeyAction)}
        style={{ ...inputStyle, maxWidth: 'none' }}
      >
        {DEVICE_KEY_ACTIONS.map(action => (
          <option key={action} value={action}>
            {t(`settings.deviceKeys.actions.${action}`)}
          </option>
        ))}
      </select>
      <div style={{ minWidth: 0 }}>
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
        {mapping.action !== 'pasteTemplate' && mapping.action !== 'sendShortcut' && (
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
