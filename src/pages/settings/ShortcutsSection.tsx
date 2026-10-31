// 快捷键设置：开始/停止、翻译、问答、切风格、唤起 App、以及只读取消/确认提示。

import { useTranslation } from 'react-i18next';
import { ShortcutRecorder } from '../../components/ShortcutRecorder';
import {
  setDictationHotkey,
  setOpenAppHotkey,
  setQaHotkey,
  setSwitchStyleHotkey,
  setTranslationHotkey,
} from '../../lib/ipc';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { Card } from '../_atoms';
import { SettingRow, Toggle } from './shared';
import { currentPlatform } from '../../lib/hotkey';

const UNBOUND_SHORTCUT = { primary: '', modifiers: [] as string[] };

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
  ];

  return (
    <Card>
      <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>
        {t('settings.shortcuts.title')}
      </div>
      <p data-shortcut-defaults-note style={{ fontSize: 12, lineHeight: 1.6, color: 'var(--ol-ink-3)' }}>{t('shortcutDefaults.description')}</p>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, marginBottom: 6 }}>
        {capability.requiresAccessibilityPermission
          ? t('settings.shortcuts.descAcc')
          : t('settings.shortcuts.descNoAcc')}
      </div>
      <SettingRow label={t('settings.shortcuts.startStop')}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, width: '100%' }}>
          <ShortcutRecorder
            value={prefs.dictationHotkey}
            alignRecordButton allowClear
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
          alignRecordButton allowClear
          onSave={async binding => {
            await setTranslationHotkey(binding);
            await savePrefs({ ...prefs, translationHotkey: binding });
          }}
        />
      </SettingRow>
      <SettingRow label={t('selectionAsk.hotkey.title')}>
          <ShortcutRecorder
            value={prefs.qaHotkey ?? UNBOUND_SHORTCUT}
            alignRecordButton allowClear
            onSave={async binding => {
              const configured = binding.primary.trim() ? binding : null;
              await setQaHotkey(configured);
              await savePrefs({ ...prefs, qaHotkey: configured });
            }}
          />
      </SettingRow>
      <SettingRow label={t('settings.shortcuts.switchStyle')}>
        <ShortcutRecorder
          value={prefs.switchStyleHotkey}
          alignRecordButton allowClear
          onSave={async binding => {
            await setSwitchStyleHotkey(binding);
            await savePrefs({ ...prefs, switchStyleHotkey: binding });
          }}
        />
      </SettingRow>
      <SettingRow label={t('settings.shortcuts.openApp')}>
        <ShortcutRecorder
          value={prefs.openAppHotkey}
          alignRecordButton allowClear
          onSave={async binding => {
            await setOpenAppHotkey(binding);
            await savePrefs({ ...prefs, openAppHotkey: binding });
          }}
        />
      </SettingRow>
      {currentPlatform().isWindows && (
        <SettingRow label={t('shortcutDefaults.enterFinish')} desc={t('shortcutDefaults.enterFinishDesc')}>
          <Toggle on={prefs.enterFinishesRecording} onToggle={on => void savePrefs(current => ({ ...current, enterFinishesRecording: on }))} />
        </SettingRow>
      )}
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
