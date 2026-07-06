import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  FIRMWARE_OTA_REQUIRED_PUBLIC_STATES,
  FIRMWARE_OTA_TRANSPORT_BOUNDARY,
  LISTENER_OTA_V2_TRANSPORT_BOUNDARY,
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
    min_desktop_version: '1.0.0',
    channel: 'development',
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
    channel: 'development',
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
      hardware_revision: 'keyboard-v2-n16r8',
      protocol_version: 1,
      min_desktop_version: '1.0.0',
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
        model: 'keyboard-v2',
        hardware_revision: 'esp32s3-wroom-1-n16r8',
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

function listenerOtaV2Manifest(overrides: Record<string, unknown> = {}): string {
  return manifestV2({
    requirements: {
      hardware_revision: 'keyboard-v2-n16r8',
      protocol_version: 2,
      min_desktop_version: '1.0.0',
    },
    protocol: {
      name: LISTENER_OTA_V2_TRANSPORT_BOUNDARY.protocolName,
      version: 2,
      firmware_capability: LISTENER_OTA_V2_TRANSPORT_BOUNDARY.firmwareCapability,
      data_plane: LISTENER_OTA_V2_TRANSPORT_BOUNDARY.dataPlane,
      gatt: {
        service_uuid: LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.serviceUuid,
        control_uuid: LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.controlUuid,
        data_uuid: LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.dataUuid,
        status_uuid: LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.statusUuid,
        chunk_bytes: LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.defaultChunkBytes,
      },
    },
    ...overrides,
  });
}

const context = {
  desktopVersion: '1.0.0',
  expectedHardwareRevision: 'esp32s3-devkit',
};

const contextV2 = {
  desktopVersion: '1.0.0',
  expectedHardwareRevision: 'keyboard-v2-n16r8',
};

const valid = await validateFirmwareOtaPackage(manifest(), firmwareBytes, context);
assert.equal(valid.ok, true);
assert.equal(valid.firmwareSha256, firmwareSha256);
assert.equal(valid.manifest?.version, '1.2.0');

const validV2 = await validateFirmwareOtaPackage(manifestV2(), firmwareBytes, contextV2);
assert.equal(validV2.ok, true);
assert.equal(validV2.firmwareSha256, firmwareSha256);
assert.equal(validV2.manifest?.schemaVersion, 2);
assert.equal(validV2.manifest?.hardwareRevision, 'keyboard-v2-n16r8');
assert.equal(validV2.manifest?.fileName, 'firmware_ota.bin');
assert.equal(validV2.manifest?.gattChunkBytes, FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.defaultChunkBytes);
assert.equal(validV2.manifest?.recoveryInstructions.length, 2);

const validListenerOtaV2 = await validateFirmwareOtaPackage(
  listenerOtaV2Manifest(),
  firmwareBytes,
  contextV2,
);
assert.equal(validListenerOtaV2.ok, true);
assert.equal(validListenerOtaV2.manifest?.protocolName, LISTENER_OTA_V2_TRANSPORT_BOUNDARY.protocolName);
assert.equal(validListenerOtaV2.manifest?.protocolVersion, 2);
assert.equal(validListenerOtaV2.manifest?.firmwareCapability, LISTENER_OTA_V2_TRANSPORT_BOUNDARY.firmwareCapability);
assert.equal(validListenerOtaV2.manifest?.gattStatusUuid, LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.statusUuid);
assert.equal(validListenerOtaV2.manifest?.gattConfirmUuid, null);

const sameVersionListenerOtaV2 = await validateFirmwareOtaPackage(
  listenerOtaV2Manifest(),
  firmwareBytes,
  { ...contextV2, currentFirmwareVersion: '1.2.0' },
);
assert.equal(sameVersionListenerOtaV2.ok, true);
assert.deepEqual(sameVersionListenerOtaV2.warnings, []);

const olderListenerOtaV2 = await validateFirmwareOtaPackage(
  listenerOtaV2Manifest(),
  firmwareBytes,
  { ...contextV2, currentFirmwareVersion: '1.2.1' },
);
assert.equal(olderListenerOtaV2.ok, true);
assert.ok(olderListenerOtaV2.warnings.some(warning => warning.includes('older than the connected firmware version')));

const badListenerOtaV2StatusUuid = JSON.parse(listenerOtaV2Manifest()) as Record<string, unknown>;
((badListenerOtaV2StatusUuid.protocol as Record<string, unknown>).gatt as Record<string, unknown>).status_uuid =
  '710af845-6d9f-6583-0c4d-9e5b3bc309ff';
const badListenerOtaV2Status = await validateFirmwareOtaPackage(
  JSON.stringify(badListenerOtaV2StatusUuid),
  firmwareBytes,
  contextV2,
);
assert.equal(badListenerOtaV2Status.ok, false);
assert.ok(badListenerOtaV2Status.errors.some(error => error.includes('Listener OTA v2 package uses an unsupported GATT boundary')));

const validV2FastChunk = await validateFirmwareOtaPackage(
  manifestV2({
    requirements: {
      hardware_revision: 'keyboard-v2-n16r8',
      protocol_version: 1,
      min_desktop_version: '1.0.0',
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

const tooLongV2Version = await validateFirmwareOtaPackage(
  manifestV2({
    firmware: {
      project: 'voice-keyboard-firmware',
      version: '1.0.0-local-build-226-g99934ff-dirty',
      git_commit: 'a'.repeat(40),
      git_dirty: true,
      target: 'esp32s3',
      file: 'firmware_ota.bin',
      size_bytes: firmwareBytes.byteLength,
      sha256: firmwareSha256,
    },
  }),
  firmwareBytes,
  contextV2,
);
assert.equal(tooLongV2Version.ok, false);
assert.ok(tooLongV2Version.errors.some(error => error.includes('too long')));

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

assert.equal(compareVersionish('v1.0.1', '1.0.0'), 1);
assert.equal(compareVersionish('1.0.0', '1.0.0'), 0);
assert.equal(compareVersionish('1.0.0', '1.0.1'), -1);
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
const parsedListenerOtaV2Manifest = validListenerOtaV2.manifest as FirmwareOtaManifest;
const readyPreflight = evaluateFirmwareOtaPreflight({
  manifest: parsedManifest,
  desktopVersion: '1.0.0',
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

const listenerOtaV2PreflightReady = evaluateFirmwareOtaPreflight({
  manifest: parsedListenerOtaV2Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.1.0',
    capabilities: ['firmware_ota_v2'],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(listenerOtaV2PreflightReady.ok, true);

const listenerOtaV2SameVersionPreflightReady = evaluateFirmwareOtaPreflight({
  manifest: parsedListenerOtaV2Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.2.0',
    capabilities: ['firmware_ota_v2'],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(listenerOtaV2SameVersionPreflightReady.ok, true);

const listenerOtaV2DowngradeBlocked = evaluateFirmwareOtaPreflight({
  manifest: parsedListenerOtaV2Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.2.1',
    capabilities: ['firmware_ota_v2'],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(listenerOtaV2DowngradeBlocked.ok, false);
assert.deepEqual(listenerOtaV2DowngradeBlocked.blockers.map(item => item.code), ['downgrade']);

const listenerOtaV2PreflightRequiresCapability = evaluateFirmwareOtaPreflight({
  manifest: parsedListenerOtaV2Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.1.0',
    capabilities: ['firmware_ota_v1'],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(listenerOtaV2PreflightRequiresCapability.ok, false);
assert.deepEqual(
  listenerOtaV2PreflightRequiresCapability.blockers.map(item => item.code),
  ['missingCapability'],
);

const unknownDeviceStatus = evaluateFirmwareOtaPreflight({
  manifest: parsedV2Manifest,
  desktopVersion: '1.0.0',
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
  desktopVersion: '1.0.0',
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

const unavailableBatteryStatus = evaluateFirmwareOtaPreflight({
  manifest: parsedV2Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.2.0',
    capabilities: ['firmware_ota_v1'],
    batteryPercent: null,
    usbPowered: false,
  },
});
assert.equal(unavailableBatteryStatus.ok, true);

const blockedPreflight = evaluateFirmwareOtaPreflight({
  manifest: parsedManifest,
  desktopVersion: '1.0.0',
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
assert.equal(LISTENER_OTA_V2_TRANSPORT_BOUNDARY.protocolName, 'listener_ble_ota_v2');
assert.equal(LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.serviceUuid, '710af845-6d9f-6583-0c4d-9e5b3bc3092a');
assert.equal(LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.controlUuid, '710af845-6d9f-6583-0c4d-9e5b3bc3092b');
assert.equal(LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.dataUuid, '710af845-6d9f-6583-0c4d-9e5b3bc3092c');
assert.equal(LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.statusUuid, '710af845-6d9f-6583-0c4d-9e5b3bc3092b');
assert.equal(LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.maxChunkBytes, 500);
assert.ok(FIRMWARE_OTA_TRANSPORT_BOUNDARY.notDataPlane.every(item => item.includes('BLE')));

const rustFirmwareOtaSource = readFileSync('src-tauri/src/firmware_ota.rs', 'utf8');
for (const expected of [
  LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.serviceUuid,
  LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.controlUuid,
  LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.dataUuid,
  LISTENER_OTA_V2_TRANSPORT_BOUNDARY.gatt.statusUuid,
]) {
  assert.ok(
    rustFirmwareOtaSource.includes(expected),
    `Listener OTA UUID ${expected} must match src-tauri/src/firmware_ota.rs`,
  );
}

const firmwareOtaPanelSource = readFileSync('src/pages/settings/FirmwareOtaPanel.tsx', 'utf8');
assert.ok(
  !firmwareOtaPanelSource.includes('bleStatus'),
  'FirmwareOtaPanel must not auto-refresh OTA GATT snapshots from background BLE status polling',
);
assert.ok(
  /setSelectedPackage\([\s\S]*?\);\s*void refreshOtaSnapshot\(\{ protocolName: result\.manifest\.protocolName \}\);\s*dispatch\(\{ type: 'ready' \}\);/.test(firmwareOtaPanelSource),
  'selecting a firmware package should refresh the OTA snapshot once with the package protocol',
);
assert.ok(
  firmwareOtaPanelSource.includes('const OTA_PREFLIGHT_SNAPSHOT_FRESH_MS = 10_000;'),
  'OTA start should reuse a very recent package-selection preflight instead of immediately querying GATT again',
);
assert.ok(
  firmwareOtaPanelSource.includes('const [otaSnapshotFetchedAtMs, setOtaSnapshotFetchedAtMs] = useState<number | null>(null);'),
  'OTA preflight freshness must be tracked explicitly',
);
assert.ok(
  firmwareOtaPanelSource.includes('setOtaSnapshotFetchedAtMs(Date.now());'),
  'OTA preflight freshness timestamp must update whenever the UI receives a snapshot',
);
assert.ok(
  firmwareOtaPanelSource.includes('const getFreshOtaSnapshot = useCallback(() => {'),
  'OTA start must have a bounded fresh-snapshot fast path',
);
assert.ok(
  !firmwareOtaPanelSource.includes('setSelectedPackage(null)'),
  'opening/canceling/rejecting a new firmware package must not clear the previously selected shared OTA/wired package',
);
assert.ok(
  firmwareOtaPanelSource.includes('const previousPackage = selectedPackage;'),
  'firmware package selection must preserve the previous package while a new dialog/load/validation attempt is pending',
);
assert.ok(
  firmwareOtaPanelSource.includes("dispatch(previousPackage\n        ? { type: 'ready' }"),
  'failed new firmware selection should restore ready state when an older package is still selected',
);
assert.ok(
  firmwareOtaPanelSource.includes('const snapshot = getFreshOtaSnapshot() ?? await refreshOtaSnapshot({ protocolName: packageForUpdate.manifest.protocolName });'),
  'starting OTA must reuse the package-selection preflight when it is still fresh and pass the package protocol on fallback refresh',
);
assert.ok(
  firmwareOtaPanelSource.includes('withTimeout(getFirmwareOtaPreflightSnapshot({ protocolName }), rpcTimeoutMs, timeoutMessage)'),
  'OTA snapshot refresh must pass the selected package protocol and have a UI timeout so the 查询中 state cannot hang forever',
);
assert.ok(
  firmwareOtaPanelSource.includes('protocolName: result.manifest.protocolName'),
  'selecting a firmware package must run the preflight snapshot with that package protocol',
);
assert.ok(
  firmwareOtaPanelSource.includes('protocolName: packageForUpdate.manifest.protocolName'),
  'starting OTA must run any fallback preflight snapshot with the selected package protocol',
);
assert.ok(
  firmwareOtaPanelSource.includes('onRefresh={() => void refreshOtaSnapshot({ waitForFirmwareVersion: true, protocolName: selectedPackage?.manifest.protocolName ?? null })}'),
  'manual OTA snapshot refresh must remain available and use the selected package protocol',
);

const commandsSource = readFileSync('src-tauri/src/commands.rs', 'utf8');
assert.ok(
  commandsSource.includes('FIRMWARE_OTA_PREFLIGHT_SNAPSHOT_TIMEOUT'),
  'firmware OTA preflight IPC must keep an outer timeout around Windows BLE snapshot probing',
);
assert.ok(
  commandsSource.includes('tokio::time::timeout(FIRMWARE_OTA_PREFLIGHT_SNAPSHOT_TIMEOUT'),
  'firmware OTA preflight IPC timeout must wrap the blocking BLE snapshot task',
);
assert.ok(
  commandsSource.includes('protocol_name: Option<String>'),
  'firmware OTA preflight IPC must accept the selected package protocol',
);
assert.ok(
  commandsSource.includes('crate::embedded_ble::listener_ota_v2_device_snapshot()'),
  'Listener OTA v2 preflight must use the v2 snapshot path instead of the legacy OTA snapshot',
);
assert.ok(
  commandsSource.includes('confirm_listener_ota_v2_reachable(&version).await'),
  'Listener OTA v2 UI transfer confirmation must use the fast reachable-service confirmation path',
);
assert.ok(
  commandsSource.includes('FIRMWARE_OTA_LISTENER_V2_REACHABLE_CONFIRM_TIMEOUT'),
  'Listener OTA v2 UI transfer confirmation must have a bounded short timeout separate from version polling',
);
assert.ok(
  commandsSource.includes('struct FirmwareOtaConfirmOutcome'),
  'firmware OTA must record confirmation timing separately from transfer timing',
);
for (const expectedTimingField of ['transfer_elapsed_ms', 'confirm_elapsed_ms', 'total_elapsed_ms']) {
  assert.ok(
    commandsSource.includes(expectedTimingField),
    `firmware OTA transfer result must include ${expectedTimingField}`,
  );
}
assert.ok(
  commandsSource.includes('[firmware-ota] BLE OTA result transport='),
  'firmware OTA backend logs must include transfer/confirm/total timing evidence',
);
for (const expectedHeadlessTimingField of ['preflight_elapsed_ms', 'transfer_elapsed_ms', 'confirm_elapsed_ms', 'total_elapsed_ms']) {
  assert.ok(
    rustFirmwareOtaSource.includes(expectedHeadlessTimingField),
    `firmware OTA headless report must include ${expectedHeadlessTimingField}`,
  );
}
for (const removedSlowFallback of ['listener_ota_v2_snapshot_with_identity_fallback', 'merge_listener_ota_v2_snapshot_identity']) {
  assert.ok(
    !rustFirmwareOtaSource.includes(removedSlowFallback),
    `Listener OTA v2 preflight must not restore slow stable-anchor identity fallback: ${removedSlowFallback}`,
  );
}
assert.ok(
  rustFirmwareOtaSource.includes('listener_ota_v2_preflight_allows_reachable_device_without_identity_metadata'),
  'Listener OTA v2 preflight must allow reachable v2 service when Windows omits optional identity metadata',
);
assert.ok(
  rustFirmwareOtaSource.includes('confirm_listener_ota_v2_reachable_version(&expected_version).await'),
  'Listener OTA v2 headless transfer confirmation must use the fast reachable-service confirmation path',
);
assert.ok(
  rustFirmwareOtaSource.includes('LISTENER_OTA_V2_REACHABLE_CONFIRM_TIMEOUT'),
  'Listener OTA v2 headless transfer confirmation must have a bounded short timeout separate from version polling',
);

const embeddedBleSource = readFileSync('src-tauri/src/embedded_ble.rs', 'utf8');
const listenerOtaV2SnapshotStart = embeddedBleSource.indexOf('fn listener_ota_v2_device_snapshot_from_target');
const listenerOtaV2SnapshotEnd = embeddedBleSource.indexOf('fn firmware_ota_device_snapshot_from_target', listenerOtaV2SnapshotStart);
assert.ok(listenerOtaV2SnapshotStart >= 0 && listenerOtaV2SnapshotEnd > listenerOtaV2SnapshotStart);
const listenerOtaV2SnapshotBody = embeddedBleSource.slice(listenerOtaV2SnapshotStart, listenerOtaV2SnapshotEnd);
assert.ok(
  embeddedBleSource.includes('The OTA v2 service itself is the capability proof.'),
  'Listener OTA v2 snapshot must document why it skips slow optional metadata probes',
);
assert.ok(
  embeddedBleSource.includes('for cache_mode in [BluetoothCacheMode::Cached, BluetoothCacheMode::Uncached]'),
  'Listener OTA v2 discovery must try the Windows GATT cache before falling back to uncached discovery',
);
assert.ok(
  !listenerOtaV2SnapshotBody.includes('read_optional_string_characteristic_from_service'),
  'Listener OTA v2 snapshot must not probe optional readiness/capability characteristics before transfer',
);

const ipcSource = readFileSync('src/lib/ipc.ts', 'utf8');
for (const expectedTimingField of ['transferElapsedMs', 'confirmElapsedMs', 'totalElapsedMs']) {
  assert.ok(
    ipcSource.includes(expectedTimingField),
    `firmware OTA IPC type must expose ${expectedTimingField}`,
  );
}
