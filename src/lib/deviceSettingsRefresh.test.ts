import assert from 'node:assert/strict';
import { shouldRefreshDeviceSettingsOnFocus } from './deviceSettingsRefresh.ts';
import type { DeviceSettingsSnapshot, DeviceSettingsUpdateRequest } from './types.ts';

const form = {
  statusLedBrightnessPercent: 50,
  keyLedBrightnessPercent: 50,
  knobLedBrightnessPercent: 50,
  edgeLedBrightnessPercent: 50,
  pluggedLowPowerIdleMinutes: 5,
  batteryLowPowerIdleMinutes: 1,
  pluggedLowPowerEnabled: true,
  voiceAutoStartEnabled: true,
  voiceAutoStopEnabled: true,
  pluggedAutoShutdownMinutes: 0,
  batteryAutoShutdownMinutes: 10,
  bleName: 'listenerB',
} satisfies DeviceSettingsUpdateRequest;

const snapshot = { bleName: 'listenerB' } as DeviceSettingsSnapshot;

assert.equal(shouldRefreshDeviceSettingsOnFocus('idle', snapshot, form, { ...form }), true);
assert.equal(
  shouldRefreshDeviceSettingsOnFocus('idle', snapshot, { ...form, bleName: 'unsaved-name' }, form),
  false,
);
assert.equal(shouldRefreshDeviceSettingsOnFocus('saving', snapshot, form, form), false);
assert.equal(shouldRefreshDeviceSettingsOnFocus('error', null, form, null), true);

console.log('device settings focus-refresh tests passed');
