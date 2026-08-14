import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import {
  DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT,
  DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
  DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED,
  DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES,
  DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
  LEGACY_DEVICE_STATUS_KEY_LED_BRIGHTNESS_DEFAULT_PERCENT,
} from './deviceSettingsDefaults.ts';
import { applyMockDeviceSettingsWrite } from './deviceSettingsMock.ts';
import type {
  DeviceSettingsSnapshot,
  DeviceSettingsUpdateRequest,
  UserPreferences,
} from './types.ts';

const minutesFromMs = (value: number) => Math.floor(value / 60_000);

const initial: DeviceSettingsSnapshot = {
  schema: 'listener.device_settings.v1',
  connected: true,
  writeSupported: true,
  source: 'mock',
  statusLedBrightnessPercent: DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
  keyLedBrightnessPercent: DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT,
  knobLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  edgeLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  ledZoneBrightnessSupported: true,
  lowPowerIdleMinutes: DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES,
  pluggedLowPowerIdleMinutes: DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES,
  batteryLowPowerIdleMinutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
  pluggedLowPowerEnabled: DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED,
  voiceAutoStartEnabled: true,
  voiceAutoStopEnabled: true,
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
    voiceAutoStartEnabled: snapshot.voiceAutoStartEnabled,
    voiceAutoStopEnabled: snapshot.voiceAutoStopEnabled,
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
  'Type frontend mock/default status LED brightness must start at 50 percent',
);
assert.equal(
  initial.keyLedBrightnessPercent,
  DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT,
  'Type frontend mock/default key LED brightness must start at 80 percent',
);
assert.equal(
  initial.pluggedLowPowerIdleMinutes,
  DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES,
  'Type frontend mock/default plugged low-power idle must start at three minutes',
);
assert.equal(
  initial.batteryLowPowerIdleMinutes,
  DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
  'Type frontend mock/default battery low-power idle must start at one minute',
);
assert.equal(
  initial.pluggedLowPowerEnabled,
  DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED,
  'Type frontend mock/default plugged low-power must start enabled',
);
assert.equal(initial.voiceAutoStartEnabled, true, 'voice auto-start must default to enabled');
assert.equal(initial.voiceAutoStopEnabled, true, 'voice auto-stop must default to enabled');
assert.equal(
  LEGACY_DEVICE_STATUS_KEY_LED_BRIGHTNESS_DEFAULT_PERCENT,
  50,
  'Type must keep the legacy status/key LED brightness default identifiable for migration',
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
assert.ok(
  deviceSectionSource.includes('DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT'),
  'device settings form must use the shared Type key LED default instead of a local literal',
);
assert.ok(
  deviceSectionSource.includes('DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT'),
  'device settings form must keep status LED default tied to the shared 50 percent contract',
);
assert.ok(
  deviceSectionSource.includes('return Math.max(0, Math.min(100, Math.trunc(value)));'),
  'device settings brightness percent input must truncate instead of rounding 40.x up to 41',
);
assert.ok(
  !deviceSectionSource.includes('return Math.max(0, Math.min(100, Math.round(value)));'),
  'device settings brightness percent input must not round typed values',
);
const percentNumberStart = deviceSectionSource.indexOf('function PercentNumber');
const percentSliderStart = deviceSectionSource.indexOf('function PercentSlider');
assert.ok(
  percentNumberStart >= 0 && percentSliderStart > percentNumberStart,
  'device settings source must expose a separate percent number input before the slider input',
);
const percentNumberSource = deviceSectionSource.slice(percentNumberStart, percentSliderStart);
assert.ok(
  percentNumberSource.includes('const [draft, setDraft] = useState'),
  'device settings percent number input must keep a local draft while editing',
);
assert.ok(
  percentNumberSource.includes('value={draft}'),
  'device settings percent number input must render the editable draft instead of forcing the clamped value while typing',
);
assert.ok(
  percentNumberSource.includes('trimmed.length === 0') && percentNumberSource.includes('return;'),
  'device settings percent number input must allow a temporarily empty draft while deleting digits',
);
assert.ok(
  !percentNumberSource.includes('Number(event.target.value)'),
  'device settings percent number input must not coerce an empty field to 0 while the user is deleting',
);
assert.ok(
  deviceSectionSource.includes('onKeyDown={handleSettingsKeyDown}') &&
    deviceSectionSource.includes('target.blur();') &&
    deviceSectionSource.includes('void save();'),
  'device settings inputs must submit the existing write path on Enter',
);
assert.ok(
  deviceSectionSource.includes('setForm(snapshotToForm(value));'),
  'device settings save must display firmware readback values instead of blindly echoing the submitted form',
);
assert.ok(
  !deviceSectionSource.includes('setForm(submittedForm);'),
  'device settings save must not mask write/readback mismatches by keeping the submitted form',
);
assert.ok(
  deviceSectionSource.includes("const [wakePhraseDraft, setWakePhraseDraft] = useState('');") &&
    deviceSectionSource.includes('const commitWakePhrase = async () =>'),
  'wake phrase editing must keep a local draft and expose an explicit commit path',
);
assert.ok(
  deviceSectionSource.includes('setWakePhraseDraft(event.target.value);') &&
    !deviceSectionSource.includes("onChange={event => void savePrefs(current => ({\n                      ...current,\n                      voiceWakePhrase: event.target.value,"),
  'wake phrase keystrokes must edit only the local draft instead of activating partial phrases',
);
assert.ok(
  deviceSectionSource.includes("if (event.key === 'Enter')") &&
    deviceSectionSource.includes('void commitWakePhrase();') &&
    deviceSectionSource.includes("t('settings.recording.wakePhraseApply'"),
  'wake phrase changes must activate only on Enter or the explicit Apply command',
);
assert.ok(
  deviceSectionSource.includes('voiceprint?.requiresReenrollment') &&
    deviceSectionSource.includes("t('settings.recording.voiceprintOpenGateDesc'"),
  'voiceprint UI must distinguish phrase-change re-enrollment from the open-to-any-speaker state',
);
const voiceprintWizardSource = readFileSync('src/pages/settings/VoiceprintEnrollmentWizard.tsx', 'utf8');
assert.ok(
  deviceSectionSource.includes('<VoiceprintEnrollmentWizard') &&
    deviceSectionSource.includes('cancelVoiceprintEnrollment'),
  'voiceprint enrollment must use the dedicated cancellable wizard instead of an inline timer',
);
assert.ok(
  voiceprintWizardSource.includes('status?.signalLevel') &&
    voiceprintWizardSource.includes('status?.stepSpeechMs') &&
    voiceprintWizardSource.includes('voiceprintWizardFeedback'),
  'voiceprint wizard must render live signal and per-step speech evidence',
);
assert.ok(
  voiceprintWizardSource.includes('const STEP_TARGET_MS = [600, 600, 600] as const;') &&
    voiceprintWizardSource.includes('Array.from({ length: 3 }') &&
    voiceprintWizardSource.includes('total: 3') &&
    !voiceprintWizardSource.includes('voiceprintWizardNaturalStep') &&
    !voiceprintWizardSource.includes('voiceprintWizardSayNaturally'),
  'voiceprint enrollment must stay a three-phrase flow without a fourth natural-speech step',
);
assert.ok(
  voiceprintWizardSource.includes("phase === 'success'") &&
    voiceprintWizardSource.includes("phase === 'error'"),
  'voiceprint wizard must give explicit completion and retry outcomes',
);

const ipcSource = readFileSync('src/lib/ipc.ts', 'utf8');
const commandsSource = [
  'src-tauri/src/commands/mod.rs',
  'src-tauri/src/commands/device/settings.rs',
  'src-tauri/src/commands/device/ble.rs',
  'src-tauri/src/commands/device/firmware.rs',
]
  .map((p) => readFileSync(p, 'utf8'))
  .join('\n');
assert.ok(
  ipcSource.includes('LEGACY_DEVICE_STATUS_KEY_LED_BRIGHTNESS_DEFAULT_PERCENT'),
  'Type IPC normalization must recognize the old status/key LED 50 default',
);
assert.match(
  ipcSource,
  /removeFillerWords:\s*true,[\s\S]*sendKeyAfterDictation:\s*false,/,
  'Type defaults must remove filler words while keeping post-dictation submission disabled',
);
assert.match(
  ipcSource,
  /voiceAutoStartEnabled:\s*true,[\s\S]*voiceAutoStopEnabled:\s*true,/,
  'Type mock device settings must default both voice automation controls to enabled',
);
assert.match(
  deviceSectionSource,
  /voiceAutoStartEnabled:\s*true,[\s\S]*voiceAutoStopEnabled:\s*true,/,
  'the device settings form must render both voice automation controls enabled before readback',
);
assert.ok(
  ipcSource.includes('deviceLedBrightness102DefaultMigrated'),
  'Type IPC normalization must persist the 1.0.2 LED brightness migration marker',
);
assert.ok(
  ipcSource.includes('deviceStatusLedBrightnessPercent = DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT'),
  'Type IPC normalization must map legacy status/key defaults through the shared current defaults',
);
assert.ok(
  ipcSource.includes('deviceKeyLedBrightnessPercent = DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT'),
  'Type IPC normalization must map legacy key defaults through the shared current defaults',
);
assert.ok(
  ipcSource.includes('deviceLowPowerIdleMinutes: deviceBatteryLowPowerIdleMinutes'),
  'Type IPC normalization must keep the legacy low-power field as a battery mirror',
);
assert.ok(
  ipcSource.includes('pluggedLowPowerIdleMinutes: nextPrefs.devicePluggedLowPowerIdleMinutes'),
  'Type mock settings save must keep plugged low-power minutes separate',
);
assert.ok(
  ipcSource.includes('batteryLowPowerIdleMinutes: nextPrefs.deviceBatteryLowPowerIdleMinutes'),
  'Type mock settings save must keep battery low-power minutes separate',
);
assert.ok(
  !ipcSource.includes('pluggedLowPowerIdleMinutes: nextPrefs.deviceLowPowerIdleMinutes'),
  'Type mock settings save must not merge plugged low-power minutes through the legacy field',
);
assert.match(
  commandsSource,
  /prefs\.device_plugged_low_power_idle_minutes\s*=\s*request\.plugged_low_power_idle_minutes;/,
  'Type backend settings persistence must store the exact plugged low-power minute request',
);
assert.match(
  commandsSource,
  /prefs\.device_battery_low_power_idle_minutes\s*=\s*request\.battery_low_power_idle_minutes;/,
  'Type backend settings persistence must store the exact battery low-power minute request',
);
assert.match(
  commandsSource,
  /device_setting_assignment\(\s*"plugged_low_power_idle_minutes",\s*request\.plugged_low_power_idle_minutes,/,
  'Type backend must send plugged low-power minutes to firmware without rounding or second conversion',
);
assert.match(
  commandsSource,
  /device_setting_assignment\(\s*"battery_low_power_idle_minutes",\s*request\.battery_low_power_idle_minutes,/,
  'Type backend must send battery low-power minutes to firmware without rounding or second conversion',
);
assert.match(
  commandsSource,
  /"plugged_low_power_idle_minutes"\s*=>\s*"plm"/,
  'Type backend compact settings writes must keep plugged low-power minutes as an exact minute token',
);
assert.match(
  commandsSource,
  /"battery_low_power_idle_minutes"\s*=>\s*"blm"/,
  'Type backend compact settings writes must keep battery low-power minutes as an exact minute token',
);
assert.match(
  commandsSource,
  /snapshot\.plugged_low_power_idle_minutes\s*!=\s*request\.plugged_low_power_idle_minutes/,
  'Type backend readback mismatch detection must compare plugged low-power minutes exactly',
);
assert.match(
  commandsSource,
  /snapshot\.battery_low_power_idle_minutes\s*!=\s*request\.battery_low_power_idle_minutes/,
  'Type backend readback mismatch detection must compare battery low-power minutes exactly',
);

const splitLowPowerWrite = applyMockDeviceSettingsWrite(
  currentSettings,
  initial,
  requestFromSnapshot(initial, {
    pluggedLowPowerIdleMinutes: 12,
    batteryLowPowerIdleMinutes: 7,
    pluggedLowPowerEnabled: true,
  }),
  '2026-06-30T00:02:30.000Z',
);
assert.equal(
  splitLowPowerWrite.settings.devicePluggedLowPowerIdleMinutes,
  12,
  'Type settings persistence must keep plugged low-power minutes exact',
);
assert.equal(
  splitLowPowerWrite.settings.deviceBatteryLowPowerIdleMinutes,
  7,
  'Type settings persistence must keep battery low-power minutes exact',
);
assert.equal(
  splitLowPowerWrite.snapshot.pluggedLowPowerIdleMinutes,
  12,
  'Type device settings snapshot must report plugged low-power minutes exact',
);
assert.equal(
  splitLowPowerWrite.snapshot.batteryLowPowerIdleMinutes,
  7,
  'Type device settings snapshot must report battery low-power minutes exact',
);

for (const minutes of [0, 1, 3, 12, 37]) {
  const deviceSettingsWrite = applyMockDeviceSettingsWrite(
    currentSettings,
    initial,
    requestFromSnapshot(
      initial,
      {
        statusLedBrightnessPercent: 67,
        keyLedBrightnessPercent: 68,
        pluggedLowPowerIdleMinutes: minutes,
        batteryLowPowerIdleMinutes: minutes,
        pluggedLowPowerEnabled: minutes > 0,
      },
    ),
    '2026-06-30T00:03:00.000Z',
  ).snapshot;
  assert.equal(
    deviceSettingsWrite.statusLedBrightnessPercent,
    67,
    'Type device settings write must report the written status LED brightness',
  );
  assert.equal(
    deviceSettingsWrite.keyLedBrightnessPercent,
    68,
    'Type device settings write must report the written key LED brightness',
  );
  assert.equal(
    deviceSettingsWrite.pluggedLowPowerIdleMinutes,
    minutes,
    'Type device settings write must keep plugged low-power minutes exact',
  );
  assert.equal(
    deviceSettingsWrite.batteryLowPowerIdleMinutes,
    minutes,
    'Type device settings write must keep battery low-power minutes exact',
  );
}

for (const profile of [
  { status: 17, key: 83, knob: 41, edge: 96, plugged: 11, battery: 29, shutdown: 5 },
  { status: 99, key: 7, knob: 64, edge: 22, plugged: 143, battery: 2, shutdown: 1440 },
  { status: 4, key: 100, knob: 1, edge: 73, plugged: 0, battery: 37, shutdown: 0 },
]) {
  const write = applyMockDeviceSettingsWrite(
    currentSettings,
    initial,
    requestFromSnapshot(
      initial,
      {
        statusLedBrightnessPercent: profile.status,
        keyLedBrightnessPercent: profile.key,
        knobLedBrightnessPercent: profile.knob,
        edgeLedBrightnessPercent: profile.edge,
        pluggedLowPowerIdleMinutes: profile.plugged,
        batteryLowPowerIdleMinutes: profile.battery,
        pluggedLowPowerEnabled: profile.plugged > 0,
        batteryAutoShutdownMinutes: profile.shutdown,
      },
    ),
    '2026-07-08T00:00:00.000Z',
  );
  assert.equal(write.settings.deviceStatusLedBrightnessPercent, profile.status, 'status brightness must persist exact profile value');
  assert.equal(write.settings.deviceKeyLedBrightnessPercent, profile.key, 'key brightness must persist exact profile value');
  assert.equal(write.settings.deviceKnobLedBrightnessPercent, profile.knob, 'knob brightness must persist exact profile value');
  assert.equal(write.settings.deviceEdgeLedBrightnessPercent, profile.edge, 'edge brightness must persist exact profile value');
  assert.equal(write.settings.devicePluggedLowPowerIdleMinutes, profile.plugged, 'plugged low-power minutes must persist exact profile value');
  assert.equal(write.settings.deviceBatteryLowPowerIdleMinutes, profile.battery, 'battery low-power minutes must persist exact profile value');
  assert.equal(write.settings.deviceBatteryAutoShutdownMinutes, profile.shutdown, 'battery auto-shutdown minutes must persist exact profile value');
  assert.equal(write.snapshot.statusLedBrightnessPercent, profile.status, 'status brightness snapshot must read back exact profile value');
  assert.equal(write.snapshot.keyLedBrightnessPercent, profile.key, 'key brightness snapshot must read back exact profile value');
  assert.equal(write.snapshot.knobLedBrightnessPercent, profile.knob, 'knob brightness snapshot must read back exact profile value');
  assert.equal(write.snapshot.edgeLedBrightnessPercent, profile.edge, 'edge brightness snapshot must read back exact profile value');
  assert.equal(write.snapshot.pluggedLowPowerIdleMinutes, profile.plugged, 'plugged low-power snapshot must read back exact profile value');
  assert.equal(write.snapshot.batteryLowPowerIdleMinutes, profile.battery, 'battery low-power snapshot must read back exact profile value');
  assert.equal(write.snapshot.batteryAutoShutdownMs, profile.shutdown * 60_000, 'battery auto-shutdown snapshot must use the exact requested minutes');
}

const voiceAutomationWrite = applyMockDeviceSettingsWrite(
  currentSettings,
  initial,
  requestFromSnapshot(initial, {
    voiceAutoStartEnabled: false,
    voiceAutoStopEnabled: false,
  }),
  '2026-07-24T00:00:00.000Z',
);
assert.equal(voiceAutomationWrite.snapshot.voiceAutoStartEnabled, false);
assert.equal(voiceAutomationWrite.snapshot.voiceAutoStopEnabled, false);
