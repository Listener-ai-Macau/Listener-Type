import assert from 'node:assert/strict';
import {
  FIRMWARE_OTA_REQUIRED_PUBLIC_STATES,
  FIRMWARE_OTA_TRANSPORT_BOUNDARY,
  compareVersionish,
  evaluateFirmwareOtaPreflight,
  firmwareOtaConfirmedVersionMatches,
  firmwareOtaConfirmedVersionLooksRolledBack,
  firmwareOtaFailureNextStep,
  firmwareOtaReducer,
  firmwareOtaRollbackVersionFromText,
  firmwareOtaVersionNotConfirmedAction,
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
        chunk_bytes: 500,
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

function manifestV2(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    schema_version: 2,
    created_at_utc: '2026-05-26T00:00:00Z',
    channel: 'internal-test',
    firmware: {
      project: 'voice-keyboard-firmware',
      version: '1.2.0',
      git_commit: 'a'.repeat(40),
      git_dirty: false,
      target: 'esp32s3',
      file: 'firmware_ota.bin',
      size_bytes: firmwareBytes.byteLength,
      sha256: firmwareSha256,
    },
    requirements: {
      hardware_revision: 'keyboard-v1',
      protocol_version: 1,
      min_desktop_version: '1.3.3',
    },
    ble_identity: {
      name: 'Listener Voice Keyboard',
      appearance: '0x03C1',
      hid_service_uuid: '1812',
      audio_service_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3091a',
      audio_notify_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3091c',
      readiness_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3091d',
      capabilities_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3091e',
      dis: {
        manufacturer: 'Listener',
        model: 'keyboard-v1',
        hardware_revision: 'esp32s3-devkit',
        firmware_revision: '1.2.0',
        software_revision_protocol: '1',
      },
    },
    rollback: {
      supported: true,
      method: 'esp_idf_bootloader_rollback',
      instructions: 'The bootloader returns to the previous slot if pending verify fails.',
    },
    recovery: {
      factory_reflash: 'Use the USB factory package from the same firmware release.',
      serial_commands: 'Open the serial monitor and run the recovery commands from the firmware bundle.',
    },
    ...overrides,
  });
}

const context = {
  desktopVersion: '1.3.3',
  expectedHardwareRevision: 'esp32s3-devkit',
};

const contextV2 = {
  desktopVersion: '1.3.3',
  expectedHardwareRevision: 'keyboard-v1',
};

const valid = await validateFirmwareOtaPackage(manifest(), firmwareBytes, context);
assert.equal(valid.ok, true);
assert.equal(valid.firmwareSha256, firmwareSha256);
assert.equal(valid.manifest?.version, '1.2.0');

const validV2 = await validateFirmwareOtaPackage(manifestV2(), firmwareBytes, contextV2);
assert.equal(validV2.ok, true);
assert.equal(validV2.firmwareSha256, firmwareSha256);
assert.equal(validV2.manifest?.schemaVersion, 2);
assert.equal(validV2.manifest?.hardwareRevision, 'keyboard-v1');
assert.equal(validV2.manifest?.fileName, 'firmware_ota.bin');
assert.equal(validV2.manifest?.gattChunkBytes, FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.defaultChunkBytes);
assert.equal(validV2.manifest?.recoveryInstructions.length, 2);

const validV2FastChunk = await validateFirmwareOtaPackage(
  manifestV2({
    requirements: {
      hardware_revision: 'keyboard-v1',
      protocol_version: 1,
      min_desktop_version: '1.3.3',
      gatt_chunk_bytes: 500,
    },
  }),
  firmwareBytes,
  contextV2,
);
assert.equal(validV2FastChunk.ok, true);
assert.equal(validV2FastChunk.manifest?.gattChunkBytes, FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.maxChunkBytes);

const missingV2BleIdentity = JSON.parse(manifestV2()) as Record<string, unknown>;
delete missingV2BleIdentity.ble_identity;
const badV2BleIdentity = await validateFirmwareOtaPackage(
  JSON.stringify(missingV2BleIdentity),
  firmwareBytes,
  contextV2,
);
assert.equal(badV2BleIdentity.ok, false);
assert.ok(badV2BleIdentity.errors.some(error => error.includes('ble_identity')));

const badV2Rollback = await validateFirmwareOtaPackage(
  manifestV2({ rollback: { supported: true, method: 'esp_idf_bootloader_rollback' } }),
  firmwareBytes,
  contextV2,
);
assert.equal(badV2Rollback.ok, false);
assert.ok(badV2Rollback.errors.some(error => error.includes('rollback.instructions')));

const badV2Recovery = await validateFirmwareOtaPackage(
  manifestV2({ recovery: { factory_reflash: 'Use USB factory reflash.' } }),
  firmwareBytes,
  contextV2,
);
assert.equal(badV2Recovery.ok, false);
assert.ok(badV2Recovery.errors.some(error => error.includes('recovery.serial_commands')));

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
        chunk_bytes: 500,
      },
    },
  }),
  firmwareBytes,
  context,
);
assert.equal(badGatt.ok, false);
assert.ok(badGatt.errors.some(error => error.includes('GATT boundary')));

const badGattChunk = await validateFirmwareOtaPackage(
  manifest({
    protocol: {
      name: 'listener_ble_ota',
      version: 1,
      firmware_capability: 'firmware_ota_v1',
      data_plane: 'dedicated OTA GATT service; never BLE audio or HID',
      gatt: {
        service_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092a',
        control_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092b',
        data_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092c',
        chunk_bytes: 501,
      },
    },
  }),
  firmwareBytes,
  context,
);
assert.equal(badGattChunk.ok, false);
assert.ok(badGattChunk.errors.some(error => error.includes('chunk size')));

assert.equal(compareVersionish('v1.3.3', '1.3.2'), 1);
assert.equal(compareVersionish('1.3.3', '1.3.3'), 0);
assert.equal(compareVersionish('1.3.3', '1.4.0'), -1);
assert.equal(firmwareOtaConfirmedVersionMatches('v1.2.0', '1.2.0'), true);
assert.equal(firmwareOtaConfirmedVersionMatches('1.2.0-dev', '1.2.0'), false);
assert.equal(firmwareOtaConfirmedVersionMatches(null, '1.2.0'), false);
assert.equal(firmwareOtaConfirmedVersionLooksRolledBack('v1.1.0', 'v1.2.0'), true);
assert.equal(firmwareOtaConfirmedVersionLooksRolledBack('v1.2.0', 'v1.2.0'), false);
assert.equal(firmwareOtaConfirmedVersionLooksRolledBack('v1.3.0', 'v1.2.0'), false);
assert.equal(firmwareOtaConfirmedVersionLooksRolledBack(null, 'v1.2.0'), false);
assert.equal(firmwareOtaRollbackVersionFromText('finish failed; device reports firmware v1.1.0 after reboot', 'v1.2.0'), 'v1.1.0');
assert.equal(firmwareOtaRollbackVersionFromText('device reports firmware v1.3.0 after reboot', 'v1.2.0'), null);
assert.equal(firmwareOtaVersionNotConfirmedAction('v1.1.0', 'v1.2.0').type, 'rolledBack');
assert.equal(firmwareOtaVersionNotConfirmedAction('v1.3.0', 'v1.2.0').type, 'failed');
assert.equal(firmwareOtaVersionNotConfirmedAction(null, 'v1.2.0').type, 'failed');

const parsedManifest = valid.manifest as FirmwareOtaManifest;
const parsedV2Manifest = validV2.manifest as FirmwareOtaManifest;
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

const unknownDeviceStatus = evaluateFirmwareOtaPreflight({
  manifest: parsedV2Manifest,
  desktopVersion: '1.3.3',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: null,
    firmwareVersion: '1.1.0',
    capabilities: ['firmware_ota_v1'],
    batteryPercent: 80,
    usbPowered: true,
  },
});
assert.equal(unknownDeviceStatus.ok, true);

const unknownPowerStatus = evaluateFirmwareOtaPreflight({
  manifest: parsedV2Manifest,
  desktopVersion: '1.3.3',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: null,
    firmwareVersion: null,
    capabilities: ['firmware_ota_v1'],
    batteryPercent: null,
    usbPowered: null,
  },
});
assert.equal(unknownPowerStatus.ok, true);

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
assert.equal(FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.defaultChunkBytes, 500);
assert.equal(FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.maxChunkBytes, 500);
assert.ok(FIRMWARE_OTA_TRANSPORT_BOUNDARY.notDataPlane.every(item => item.includes('BLE')));
