// 快捷键设置：开始/停止、翻译、问答、切风格、唤起 App、以及只读取消/确认提示。

import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ShortcutRecorder } from '../../components/ShortcutRecorder';
import { SelectLite } from '../../components/ui/SelectLite';
import { defaultAppShortcutModifiers, defaultQaShortcut, formatComboLabel } from '../../lib/hotkey';
import {
  listInstalledApplications,
  setDictationHotkey,
  setOpenAppHotkey,
  setQaHotkey,
  setSwitchStyleHotkey,
  setTranslationHotkey,
} from '../../lib/ipc';
import type {
  DeviceCustomKeyAction,
  DeviceCustomKeyAppPage,
  DeviceCustomKeyGesture,
  DeviceCustomKeyId,
  DeviceCustomKeys,
  DeviceCustomKeyMapping,
  InstalledApplication,
  ShortcutBinding,
} from '../../lib/types';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { Card } from '../_atoms';
import { inputStyle, SettingRow } from './shared';

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
  'settingsProviders',
  'settingsShortcuts',
  'settingsPermissions',
  'settingsLanguage',
  'settingsAdvanced',
];

const EXTERNAL_APP_MANUAL_VALUE = '__manual_external_app__';

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

  const readonlyRows: Array<[string, string]> = [
    [t('settings.shortcuts.cancel'), 'Esc'],
    [t('settings.shortcuts.confirm'), t('settings.shortcuts.confirmHint')],
  ];
  return (
    <Card>
      <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>
        {t('settings.shortcuts.title')}
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, marginBottom: 6 }}>
        {capability.requiresAccessibilityPermission
          ? t('settings.shortcuts.descAcc')
          : t('settings.shortcuts.descNoAcc')}
      </div>
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

export function DeviceKeysPanel() {
  const { t } = useTranslation();
  const { prefs, updatePrefs: savePrefs } = useHotkeySettings();
  const [installedApps, setInstalledApps] = useState<InstalledApplication[]>([]);
  const [installedAppsLoading, setInstalledAppsLoading] = useState(false);
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

  if (!prefs) {
    return (
      <Card>
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>{t('common.loading')}</div>
      </Card>
    );
  }

  return (
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
    </Card>
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
      appPage: action === 'openApp' ? mapping.appPage ?? 'settingsShortcuts' : mapping.appPage,
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
            value={mapping.appPage ?? 'settingsShortcuts'}
            onChange={value => void onChange({ ...mapping, appPage: value as DeviceCustomKeyAppPage })}
            options={DEVICE_KEY_APP_PAGES.map(page => ({
              value: page,
              label: t(`settings.deviceKeys.appPages.${page}`),
            }))}
            style={{ ...inputStyle, maxWidth: 'none' }}
            ariaLabel={t('settings.deviceKeys.appPageSelectAria')}
          />
        )}
        {mapping.action === 'openExternalApp' && (
          <div style={{ display: 'grid', gridTemplateColumns: 'minmax(130px, 220px) minmax(0, 1fr)', gap: 8, width: '100%' }}>
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
