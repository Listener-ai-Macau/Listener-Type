import type {
  DeviceSettingsSnapshot,
  DeviceSettingsUpdateRequest,
  UserPreferences,
} from './types.ts';

export function applyMockDeviceSettingsWrite(
  currentSettings: UserPreferences,
  currentSnapshot: DeviceSettingsSnapshot,
  request: DeviceSettingsUpdateRequest,
  nowIso: string = new Date().toISOString(),
): { settings: UserPreferences; snapshot: DeviceSettingsSnapshot } {
  const batteryAutoShutdownMs = Math.round(request.batteryAutoShutdownMinutes * 60 * 1000);
  const pluggedAutoShutdownMs = 0;
  const settings: UserPreferences = {
    ...currentSettings,
    deviceStatusLedBrightnessPercent: request.statusLedBrightnessPercent,
    deviceKeyLedBrightnessPercent: request.keyLedBrightnessPercent,
    deviceKnobLedBrightnessPercent: request.knobLedBrightnessPercent,
    deviceEdgeLedBrightnessPercent: request.edgeLedBrightnessPercent,
    deviceLowPowerIdleMinutes: request.batteryLowPowerIdleMinutes,
    devicePluggedLowPowerEnabled: request.pluggedLowPowerEnabled,
    deviceBatteryAutoShutdownMinutes: request.batteryAutoShutdownMinutes,
    deviceBleName: request.bleName,
  };
  const snapshot: DeviceSettingsSnapshot = {
    ...currentSnapshot,
    statusLedBrightnessPercent: request.statusLedBrightnessPercent,
    keyLedBrightnessPercent: request.keyLedBrightnessPercent,
    knobLedBrightnessPercent: request.knobLedBrightnessPercent,
    edgeLedBrightnessPercent: request.edgeLedBrightnessPercent,
    ledZoneBrightnessSupported: true,
    lowPowerIdleMinutes: currentSnapshot.activePowerSource === 'plugged'
      ? request.pluggedLowPowerIdleMinutes
      : request.batteryLowPowerIdleMinutes,
    pluggedLowPowerIdleMinutes: request.pluggedLowPowerIdleMinutes,
    batteryLowPowerIdleMinutes: request.batteryLowPowerIdleMinutes,
    pluggedLowPowerEnabled: request.pluggedLowPowerEnabled,
    pluggedAutoShutdownMs,
    batteryAutoShutdownMs,
    bleName: request.bleName,
    bleNamePendingRestart: currentSnapshot.bleName !== request.bleName,
    source: 'mock',
    detail: 'Browser preview mock. Tauri builds use the firmware DEVICE command contract.',
    lastUpdatedAt: nowIso,
  };
  return { settings, snapshot };
}
