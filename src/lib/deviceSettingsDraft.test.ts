import assert from 'node:assert/strict';
import { DeviceSettingsDraft } from './deviceSettingsDraft.ts';
import type { DeviceSettingsUpdateRequest } from './types.ts';
const boardA: DeviceSettingsUpdateRequest = { bleName: 'Alice', statusLedBrightnessPercent: 20, keyLedBrightnessPercent: 10, knobLedBrightnessPercent: 30, edgeLedBrightnessPercent: 40, pluggedLowPowerIdleMinutes: 5, batteryLowPowerIdleMinutes: 2, pluggedLowPowerEnabled: true, voiceAutoStartEnabled: true, voiceAutoStopEnabled: true, pluggedAutoShutdownMinutes: 0, batteryAutoShutdownMinutes: 15 };
// Editing works without any read; late read and retry must retain those edits.
const draft = new DeviceSettingsDraft();
draft.edit({ statusLedBrightnessPercent: 75 });
assert.equal(draft.merge(boardA).statusLedBrightnessPercent, 75);
// The display's default name and unedited defaults must never rename/reset a board.
assert.equal(draft.merge(boardA).bleName, 'Alice');
assert.equal(draft.merge(boardA).batteryAutoShutdownMinutes, 15);
const fresh = { ...boardA, batteryAutoShutdownMinutes: 25, keyLedBrightnessPercent: 99 };
assert.deepEqual(draft.merge(fresh), { ...fresh, statusLedBrightnessPercent: 75 });
// An explicit rename survives a read/retry. A new selection gets its own draft.
draft.edit({ bleName: 'My-device' });
assert.equal(draft.merge(fresh).bleName, 'My-device');
assert.equal(draft.has('bleName'), true);
const boardB = { ...fresh, bleName: 'Bob' };
assert.deepEqual(new DeviceSettingsDraft().merge(boardB), boardB);
// An intentional clear is validation input, never silently replaced by a default.
draft.edit({ bleName: '' });
assert.equal(draft.merge(boardA).bleName, '');
console.log('PASS: pre-read edits, late-read/retry preservation, untouched defaults, fresh merge, device switch, explicit name clear');
