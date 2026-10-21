import type { DeviceCustomKeyMapping, DeviceCustomKeys, DeviceKeyProfile } from './types';

export const layoutKeyIds = ['key1', 'key2', 'key3', 'key4', 'knob'] as const;
export const layoutGestures = ['deviceCustomKeys', 'deviceCustomKeyDoubleClicks', 'deviceCustomKeyLongPresses'] as const;
export const layoutActions = ['sendShortcut', 'dictation', 'openApp', 'pasteShortcut', 'openExternalApp', 'copyShortcut', 'undoShortcut', 'pasteTemplate', 'switchStyle', 'translation', 'selectionAsk', 'disabled'] as const;
export const layoutAppPages = ['overview', 'history', 'vocab', 'style', 'translation', 'selectionAsk', 'settingsRecording', 'settingsDevice', 'settingsProviders', 'settingsShortcuts', 'settingsPermissions', 'settingsLanguage', 'settingsAdvanced'] as const;
const disabledKey = (): DeviceCustomKeyMapping => ({ action: 'disabled', appPage: 'settingsDevice', externalAppPath: '', pasteTemplate: '', shortcut: null });
const disabledKeys = (): DeviceCustomKeys => Object.fromEntries(layoutKeyIds.map(id => [id, disabledKey()])) as unknown as DeviceCustomKeys;
export function defaultDeviceKeyProfile(): DeviceKeyProfile {
  const shortcut = (primary: string, modifiers: string[] = []): DeviceCustomKeyMapping => ({ ...disabledKey(), action: 'sendShortcut', shortcut: { primary, modifiers } });
  return { schema: 1, hid: [], deviceCustomKeys: { key1: shortcut('RightControl'), key2: shortcut('C', ['ctrl']), key3: shortcut('V', ['ctrl']), key4: shortcut('Enter'), knob: { ...disabledKey(), action: 'dictation' } }, deviceCustomKeyDoubleClicks: disabledKeys(), deviceCustomKeyLongPresses: disabledKeys() };
}
export function writingDeviceKeyProfile(): DeviceKeyProfile {
  const profile = defaultDeviceKeyProfile();
  profile.deviceCustomKeys.key1 = { ...disabledKey(), action: 'pasteTemplate', pasteTemplate: 'Please make this clearer without adding facts.' };
  profile.deviceCustomKeys.key2.action = 'copyShortcut';
  profile.deviceCustomKeys.key3.action = 'pasteShortcut';
  profile.deviceCustomKeys.key4 = { ...disabledKey(), action: 'undoShortcut' };
  return profile;
}
