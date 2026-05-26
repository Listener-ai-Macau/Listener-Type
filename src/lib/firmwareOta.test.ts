import assert from 'node:assert/strict';
import {
  FIRMWARE_OTA_REQUIRED_PUBLIC_STATES,
  FIRMWARE_OTA_TRANSPORT_BOUNDARY,
  compareVersionish,
  evaluateFirmwareOtaPreflight,
  firmwareOtaFailureNextStep,
  firmwareOtaReducer,
  initialFirmwareOtaState,
  validateFirmwareOtaPackage,
  type FirmwareOtaManifest,
} from './firmwareOta.ts';

const firmwareBytes = new Uint8Array([0xe9, 1, 2, 3, 4, 5]);
const firmwareSha256 = '6d3841935f58db1c3efa67022f2d770184be6fdef93c087bca10c30e70157e84';

function manifest(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    schema_version: 1,
    package_type: 'listener-firmware-ota',
    project: 'voice-keyboard-firmware',
    version: '1.2.0',
    protocol: {
      name: 'listener_ble_ota',
      version: 1,
      firmware_capability: 'firmware_ota_v1',
      data_plane: 'dedicated OTA GATT service; never BLE audio or HID',
      gatt: {
        service_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092a',
        control_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092b',
        data_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092c',
        chunk_bytes: 180,
      },
    },
    hardware_revision: 'esp32s3-devkit',
    min_desktop_version: '1.3.3',
    channel: 'internal-test',
    file: {
      name: 'firmware_ota.bin',
      size_bytes: firmwareBytes.byteLength,
      sha256: firmwareSha256,
    },
    rollback: {
      strategy: 'esp_idf_bootloader_rollback',
      instructions: ['Rollback returns to previous slot if pending verify fails.'],
    },
    recovery: {
      instructions: ['Reconnect Bluetooth and retry, or use factory_flash over USB.'],
    },
    ...overrides,
  });
}

const context = {
  desktopVersion: '1.3.3',
  expectedHardwareRevision: 'esp32s3-devkit',
};

const valid = await validateFirmwareOtaPackage(manifest(), firmwareBytes, context);
assert.equal(valid.ok, true);
assert.equal(valid.firmwareSha256, firmwareSha256);
assert.equal(valid.manifest?.version, '1.2.0');

const badHash = await validateFirmwareOtaPackage(
  manifest({ file: { name: 'firmware_ota.bin', size_bytes: firmwareBytes.byteLength, sha256: '0'.repeat(64) } }),
  firmwareBytes,
  context,
);
assert.equal(badHash.ok, false);
assert.ok(badHash.errors.some(error => error.includes('SHA256')));

const badSize = await validateFirmwareOtaPackage(
  manifest({ file: { name: 'firmware_ota.bin', size_bytes: firmwareBytes.byteLength + 1, sha256: firmwareSha256 } }),
  firmwareBytes,
  context,
);
assert.equal(badSize.ok, false);
assert.ok(badSize.errors.some(error => error.includes('size mismatch')));

const badHardware = await validateFirmwareOtaPackage(
  manifest({ hardware_revision: 'keyboard-v2' }),
  firmwareBytes,
  context,
);
assert.equal(badHardware.ok, false);
assert.ok(badHardware.errors.some(error => error.includes('Hardware revision mismatch')));

const oldDesktop = await validateFirmwareOtaPackage(
  manifest({ min_desktop_version: '9.0.0' }),
  firmwareBytes,
  context,
);
assert.equal(oldDesktop.ok, false);
assert.ok(oldDesktop.errors.some(error => error.includes('older than required')));

const badGatt = await validateFirmwareOtaPackage(
  manifest({
    protocol: {
      name: 'listener_ble_ota',
      version: 1,
      firmware_capability: 'firmware_ota_v1',
      data_plane: 'dedicated OTA GATT service; never BLE audio or HID',
      gatt: {
        service_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc309ff',
        control_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092b',
        data_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092c',
        chunk_bytes: 180,
      },
    },
  }),
  firmwareBytes,
  context,
);
assert.equal(badGatt.ok, false);
assert.ok(badGatt.errors.some(error => error.includes('GATT boundary')));

assert.equal(compareVersionish('v1.3.3', '1.3.2'), 1);
assert.equal(compareVersionish('1.3.3', '1.3.3'), 0);
assert.equal(compareVersionish('1.3.3', '1.4.0'), -1);

const parsedManifest = valid.manifest as FirmwareOtaManifest;
const readyPreflight = evaluateFirmwareOtaPreflight({
  manifest: parsedManifest,
  desktopVersion: '1.3.3',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'esp32s3-devkit',
    firmwareVersion: '1.1.0',
    capabilities: ['firmware_ota_v1'],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(readyPreflight.ok, true);

const blockedPreflight = evaluateFirmwareOtaPreflight({
  manifest: parsedManifest,
  desktopVersion: '1.3.3',
  recordingActive: true,
  transferActive: true,
  device: {
    connected: false,
    hardwareRevision: 'keyboard-v2',
    firmwareVersion: '1.2.0',
    capabilities: [],
    batteryPercent: 9,
    usbPowered: false,
  },
});
assert.equal(blockedPreflight.ok, false);
assert.deepEqual(
  blockedPreflight.blockers.map(item => item.code),
  [
    'deviceDisconnected',
    'recordingActive',
    'transferActive',
    'hardwareMismatch',
    'missingCapability',
    'sameVersion',
    'batteryLow',
  ],
);

let state = firmwareOtaReducer(initialFirmwareOtaState, { type: 'check' });
assert.equal(state.userState, 'checking');
state = firmwareOtaReducer(state, { type: 'ready' });
assert.equal(state.userState, 'ready');
state = firmwareOtaReducer(state, { type: 'startTransfer' });
state = firmwareOtaReducer(state, { type: 'transferProgress', progress: 55 });
assert.equal(state.userState, 'transferring');
assert.equal(state.progress, 55);
state = firmwareOtaReducer(state, { type: 'transferComplete' });
assert.equal(state.userState, 'rebooting');
state = firmwareOtaReducer(state, { type: 'deviceReconnected' });
assert.equal(state.userState, 'verifying');
state = firmwareOtaReducer(state, { type: 'verified' });
assert.equal(state.userState, 'success');

const failed = firmwareOtaReducer(state, {
  type: 'failed',
  failureCode: 'bleDisconnected',
  message: 'BLE disconnected',
});
assert.equal(failed.userState, 'failed');
assert.ok(firmwareOtaFailureNextStep('bleDisconnected').includes('Reconnect'));

const rolledBack = firmwareOtaReducer(state, { type: 'rolledBack', message: 'Rolled back' });
assert.equal(rolledBack.userState, 'rolledBack');

assert.deepEqual(
  FIRMWARE_OTA_REQUIRED_PUBLIC_STATES,
  ['checking', 'ready', 'transferring', 'rebooting', 'verifying', 'success', 'failed', 'rolledBack'],
);
assert.equal(FIRMWARE_OTA_TRANSPORT_BOUNDARY.protocolName, 'listener_ble_ota');
assert.equal(FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.chunkBytes, 180);
assert.ok(FIRMWARE_OTA_TRANSPORT_BOUNDARY.notDataPlane.every(item => item.includes('BLE')));
