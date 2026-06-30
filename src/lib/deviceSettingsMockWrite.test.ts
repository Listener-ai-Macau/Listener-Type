import assert from 'node:assert/strict';

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
  statusLedBrightnessPercent: 100,
  keyLedBrightnessPercent: 100,
  knobLedBrightnessPercent: 100,
  edgeLedBrightnessPercent: 100,
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

const currentSettings = { deviceBleName: initial.bleName } as UserPreferences;
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
