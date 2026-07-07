import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import {
  DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
} from './deviceSettingsDefaults.ts';
import { applyMockDeviceSettingsWrite } from './deviceSettingsMock.ts';
import type {
  DeviceSettingsSnapshot,
  DeviceSettingsUpdateRequest,
  UserPreferences,
} from './types.ts';

const minutesFromMs = (value: number) => Math.round(value / 60_000);

const initial: DeviceSettingsSnapshot = {
  schema: 'listener.device_settings.v1',
  connected: true,
  writeSupported: true,
  source: 'mock',
  statusLedBrightnessPercent: DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
  keyLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  knobLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  edgeLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  ledZoneBrightnessSupported: true,
  lowPowerIdleMinutes: 1,
  pluggedLowPowerIdleMinutes: 1,
  batteryLowPowerIdleMinutes: 1,
  pluggedLowPowerEnabled: true,
  pluggedAutoShutdownMs: 0,
  batteryAutoShutdownMs: 10 * 60 * 1000,
  knobRotationAction: 'systemVolume',
  bleName: 'listener',
  bleNamePendingRestart: false,
  activePowerSource: 'plugged',
  batteryPercent: 82,
  detail: null,
  lastUpdatedAt: '2026-06-30T00:00:00.000Z',
};

function requestFromSnapshot(
  snapshot: DeviceSettingsSnapshot,
  overrides: Partial<DeviceSettingsUpdateRequest> = {},
): DeviceSettingsUpdateRequest {
  return {
    statusLedBrightnessPercent: snapshot.statusLedBrightnessPercent,
    keyLedBrightnessPercent: snapshot.keyLedBrightnessPercent,
    knobLedBrightnessPercent: snapshot.knobLedBrightnessPercent,
    edgeLedBrightnessPercent: snapshot.edgeLedBrightnessPercent,
    pluggedLowPowerIdleMinutes: snapshot.pluggedLowPowerIdleMinutes,
    batteryLowPowerIdleMinutes: snapshot.batteryLowPowerIdleMinutes,
    pluggedLowPowerEnabled: snapshot.pluggedLowPowerEnabled,
    pluggedAutoShutdownMinutes: minutesFromMs(snapshot.pluggedAutoShutdownMs),
    batteryAutoShutdownMinutes: minutesFromMs(snapshot.batteryAutoShutdownMs),
    bleName: snapshot.bleName,
    ...overrides,
  };
}

function nextBleName(current: string): string {
  return current === 'listener' ? 'listenerB' : 'listener';
}

const currentSettings = {
  deviceBleName: initial.bleName,
} as UserPreferences;
assert.equal(
  initial.statusLedBrightnessPercent,
  DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
  'Type frontend mock/default status LED brightness must start at 80 percent',
);
assert.equal(
  initial.keyLedBrightnessPercent,
  DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  'Type frontend mock/default non-status LED zones must start at 100 percent',
);
const sameNameWrite = applyMockDeviceSettingsWrite(
  currentSettings,
  initial,
  requestFromSnapshot(initial, {
    batteryLowPowerIdleMinutes: initial.batteryLowPowerIdleMinutes + 1,
  }),
  '2026-06-30T00:01:00.000Z',
).snapshot;

assert.equal(sameNameWrite.bleName, initial.bleName);
assert.equal(
  sameNameWrite.bleNamePendingRestart,
  false,
  'mock settings write must not request BLE re-pair when the BLE name is unchanged',
);

const renamedWrite = applyMockDeviceSettingsWrite(
  currentSettings,
  sameNameWrite,
  requestFromSnapshot(sameNameWrite, {
    bleName: nextBleName(sameNameWrite.bleName),
  }),
  '2026-06-30T00:02:00.000Z',
).snapshot;

assert.equal(
  renamedWrite.bleNamePendingRestart,
  true,
  'mock settings write should request BLE re-pair only when the BLE name changes',
);

const deviceSectionSource = readFileSync('src/pages/settings/DeviceSection.tsx', 'utf8');
assert.ok(
  deviceSectionSource.includes('Math.floor(value / 60000)'),
  'device settings form must not round millisecond readback up to the next displayed minute',
);
assert.ok(
  !deviceSectionSource.includes('Math.round(value / 60000)'),
  'device settings form must preserve the backend no-round-up minute contract',
);
assert.ok(
  deviceSectionSource.includes('DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT'),
  'device settings form must use the shared Type status LED default instead of a local literal',
);
