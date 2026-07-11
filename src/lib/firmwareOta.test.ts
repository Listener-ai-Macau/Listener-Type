import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  FIRMWARE_OTA_REQUIRED_PUBLIC_STATES,
  LISTENER_OTA_V1_TRANSPORT_BOUNDARY,
  compareVersionish,
  evaluateFirmwareOtaPreflight,
  firmwareOtaConfirmedVersionMatches,
  firmwareOtaConfirmedVersionLooksRolledBack,
  firmwareOtaFailureNextStep,
  firmwareOtaReducer,
  firmwareOtaRollbackVersionFromText,
  firmwareOtaSnapshotSatisfiesVersionRefreshFallback,
  firmwareOtaVersionNotConfirmedAction,
  initialFirmwareOtaState,
  validateFirmwareOtaPackage,
  type FirmwareOtaManifest,
} from './firmwareOta.ts';

const firmwareBytes = new Uint8Array([0xe9, 1, 2, 3, 4, 5]);
const firmwareSha256 = '6d3841935f58db1c3efa67022f2d770184be6fdef93c087bca10c30e70157e84';

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
    protocol: {
      name: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.protocolName,
      version: 1,
      firmware_capability: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability,
      data_plane: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.dataPlane,
      gatt: {
        service_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.serviceUuid,
        control_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.controlUuid,
        data_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.dataUuid,
        status_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.statusUuid,
        chunk_bytes: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.defaultChunkBytes,
      },
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

function listenerOtaV1Manifest(overrides: Record<string, unknown> = {}): string {
  return manifestV2({
    requirements: {
      hardware_revision: 'keyboard-v2-n16r8',
      protocol_version: 1,
      min_desktop_version: '1.0.0',
    },
    protocol: {
      name: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.protocolName,
      version: 1,
      firmware_capability: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability,
      data_plane: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.dataPlane,
      gatt: {
        service_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.serviceUuid,
        control_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.controlUuid,
        data_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.dataUuid,
        status_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.statusUuid,
        chunk_bytes: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.defaultChunkBytes,
      },
    },
    ...overrides,
  });
}

const contextV2 = {
  desktopVersion: '1.0.0',
  expectedHardwareRevision: 'keyboard-v2-n16r8',
};

const validV2 = await validateFirmwareOtaPackage(manifestV2(), firmwareBytes, contextV2);
assert.equal(validV2.ok, true);
assert.equal(validV2.firmwareSha256, firmwareSha256);
assert.equal(validV2.manifest?.schemaVersion, 2);
assert.equal(validV2.manifest?.hardwareRevision, 'keyboard-v2-n16r8');
assert.equal(validV2.manifest?.fileName, 'firmware_ota.bin');
assert.equal(validV2.manifest?.gattChunkBytes, LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.defaultChunkBytes);
assert.equal(validV2.manifest?.recoveryInstructions.length, 2);

const validListenerOtaV1 = await validateFirmwareOtaPackage(
  listenerOtaV1Manifest(),
  firmwareBytes,
  contextV2,
);
assert.equal(validListenerOtaV1.ok, true);
assert.equal(validListenerOtaV1.manifest?.protocolName, LISTENER_OTA_V1_TRANSPORT_BOUNDARY.protocolName);
assert.equal(validListenerOtaV1.manifest?.protocolVersion, 1);
assert.equal(validListenerOtaV1.manifest?.firmwareCapability, LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability);
assert.equal(validListenerOtaV1.manifest?.gattStatusUuid, LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.statusUuid);
assert.equal(validListenerOtaV1.manifest?.gattConfirmUuid, null);

const sameVersionListenerOtaV1 = await validateFirmwareOtaPackage(
  listenerOtaV1Manifest(),
  firmwareBytes,
  { ...contextV2, currentFirmwareVersion: '1.2.0' },
);
assert.equal(sameVersionListenerOtaV1.ok, true);
assert.deepEqual(sameVersionListenerOtaV1.warnings, []);

const olderListenerOtaV1 = await validateFirmwareOtaPackage(
  listenerOtaV1Manifest(),
  firmwareBytes,
  { ...contextV2, currentFirmwareVersion: '1.2.1' },
);
assert.equal(olderListenerOtaV1.ok, true);
assert.ok(olderListenerOtaV1.warnings.some(warning => warning.includes('older than the connected firmware version')));

const badListenerOtaV1StatusUuid = JSON.parse(listenerOtaV1Manifest()) as Record<string, unknown>;
((badListenerOtaV1StatusUuid.protocol as Record<string, unknown>).gatt as Record<string, unknown>).status_uuid =
  '710af845-6d9f-6583-0c4d-9e5b3bc309ff';
const badListenerOtaV1Status = await validateFirmwareOtaPackage(
  JSON.stringify(badListenerOtaV1StatusUuid),
  firmwareBytes,
  contextV2,
);
assert.equal(badListenerOtaV1Status.ok, false);
assert.ok(badListenerOtaV1Status.errors.some(error => error.includes('Listener OTA v1 package uses an unsupported GATT boundary')));

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
assert.equal(validV2FastChunk.manifest?.gattChunkBytes, LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.maxChunkBytes);

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
  manifestV2({
    firmware: {
      project: 'voice-keyboard-firmware', version: '1.2.0', git_commit: 'a'.repeat(40), git_dirty: false,
      target: 'esp32s3', file: 'firmware_ota.bin', size_bytes: firmwareBytes.byteLength, sha256: '0'.repeat(64),
    },
  }),
  firmwareBytes,
  contextV2,
);
assert.equal(badHash.ok, false);
assert.ok(badHash.errors.some(error => error.includes('SHA256')));

const badSize = await validateFirmwareOtaPackage(
  manifestV2({
    firmware: {
      project: 'voice-keyboard-firmware', version: '1.2.0', git_commit: 'a'.repeat(40), git_dirty: false,
      target: 'esp32s3', file: 'firmware_ota.bin', size_bytes: firmwareBytes.byteLength + 1, sha256: firmwareSha256,
    },
  }),
  firmwareBytes,
  contextV2,
);
assert.equal(badSize.ok, false);
assert.ok(badSize.errors.some(error => error.includes('size mismatch')));

const badHardware = await validateFirmwareOtaPackage(
  manifestV2({ requirements: { hardware_revision: 'keyboard-v2', protocol_version: 1, min_desktop_version: '1.0.0' } }),
  firmwareBytes,
  contextV2,
);
assert.equal(badHardware.ok, false);
assert.ok(badHardware.errors.some(error => error.includes('Hardware revision mismatch')));

const oldDesktop = await validateFirmwareOtaPackage(
  manifestV2({ requirements: { hardware_revision: 'keyboard-v2-n16r8', protocol_version: 1, min_desktop_version: '9.0.0' } }),
  firmwareBytes,
  contextV2,
);
assert.equal(oldDesktop.ok, false);
assert.ok(oldDesktop.errors.some(error => error.includes('older than required')));

const badGatt = await validateFirmwareOtaPackage(
  manifestV2({
    protocol: {
      name: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.protocolName,
      version: 1,
      firmware_capability: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability,
      data_plane: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.dataPlane,
      gatt: {
        service_uuid: '710af845-6d9f-6583-0c4d-9e5b3bc309ff',
        control_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.controlUuid,
        data_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.dataUuid,
        status_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.statusUuid,
        chunk_bytes: 500,
      },
    },
  }),
  firmwareBytes,
  contextV2,
);
assert.equal(badGatt.ok, false);
assert.ok(badGatt.errors.some(error => error.includes('GATT boundary')));

const badGattChunk = await validateFirmwareOtaPackage(
  manifestV2({
    protocol: {
      name: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.protocolName,
      version: 1,
      firmware_capability: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability,
      data_plane: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.dataPlane,
      gatt: {
        service_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.serviceUuid,
        control_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.controlUuid,
        data_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.dataUuid,
        status_uuid: LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.statusUuid,
        chunk_bytes: 501,
      },
    },
  }),
  firmwareBytes,
  contextV2,
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

const parsedV2Manifest = validV2.manifest as FirmwareOtaManifest;
const parsedListenerOtaV1Manifest = validListenerOtaV1.manifest as FirmwareOtaManifest;
const readyPreflight = evaluateFirmwareOtaPreflight({
  manifest: parsedV2Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.1.0',
    capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(readyPreflight.ok, true);

const listenerOtaV1PreflightReady = evaluateFirmwareOtaPreflight({
  manifest: parsedListenerOtaV1Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.1.0',
    capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(listenerOtaV1PreflightReady.ok, true);

const listenerOtaV1SameVersionPreflightReady = evaluateFirmwareOtaPreflight({
  manifest: parsedListenerOtaV1Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.2.0',
    capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(listenerOtaV1SameVersionPreflightReady.ok, true);

const listenerOtaV1DowngradeBlocked = evaluateFirmwareOtaPreflight({
  manifest: parsedListenerOtaV1Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.2.1',
    capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(listenerOtaV1DowngradeBlocked.ok, false);
assert.deepEqual(listenerOtaV1DowngradeBlocked.blockers.map(item => item.code), ['downgrade']);

const listenerOtaV1PreflightRequiresCapability = evaluateFirmwareOtaPreflight({
  manifest: parsedListenerOtaV1Manifest,
  desktopVersion: '1.0.0',
  recordingActive: false,
  transferActive: false,
  device: {
    connected: true,
    hardwareRevision: 'keyboard-v2-n16r8',
    firmwareVersion: '1.1.0',
    capabilities: ['unrelated_capability'],
    batteryPercent: 65,
    usbPowered: false,
  },
});
assert.equal(listenerOtaV1PreflightRequiresCapability.ok, false);
assert.deepEqual(
  listenerOtaV1PreflightRequiresCapability.blockers.map(item => item.code),
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
    capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
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
    capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
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
    capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
    batteryPercent: null,
    usbPowered: false,
  },
});
assert.equal(unavailableBatteryStatus.ok, true);

const blockedPreflight = evaluateFirmwareOtaPreflight({
  manifest: parsedV2Manifest,
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
assert.equal(LISTENER_OTA_V1_TRANSPORT_BOUNDARY.protocolName, 'denzic_ota_v1');
assert.equal(LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.serviceUuid, '1b55f597-f09c-4c7f-9529-adfa64983b06');
assert.equal(LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.controlUuid, '206c5e29-c64d-4392-8180-66463788533c');
assert.equal(LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.dataUuid, 'fbc4b0fb-6102-4bd1-abe4-e5e90a9a7e12');
assert.equal(LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.statusUuid, 'e571544a-7c41-4650-b0d6-ccebfe1db489');
assert.equal(LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.maxChunkBytes, 500);
assert.ok(LISTENER_OTA_V1_TRANSPORT_BOUNDARY.notDataPlane.every(item => item.includes('BLE')));
assert.equal(
  firmwareOtaSnapshotSatisfiesVersionRefreshFallback(
    {
      recordingActive: false,
      dictationPhase: 'idle',
      device: {
        connected: true,
        hardwareRevision: null,
        firmwareVersion: null,
        capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
      },
    },
    LISTENER_OTA_V1_TRANSPORT_BOUNDARY.protocolName,
  ),
  true,
  'Listener OTA v1 refresh fallback must accept a reachable v1 service even when DIS firmware version is unavailable',
);
assert.equal(
  firmwareOtaSnapshotSatisfiesVersionRefreshFallback(
    {
      recordingActive: false,
      dictationPhase: 'idle',
      device: {
        connected: true,
        hardwareRevision: null,
        firmwareVersion: null,
        capabilities: [LISTENER_OTA_V1_TRANSPORT_BOUNDARY.firmwareCapability],
      },
    },
    'listener_ota_v1',
  ),
  false,
  'old Listener OTA v1 protocol strings must not satisfy the package-selected refresh fallback',
);

const rustFirmwareOtaSource = readFileSync('src-tauri/src/firmware_ota.rs', 'utf8');
for (const expected of [
  'denzic_ota_core::GATT_SERVICE_UUID',
  'denzic_ota_core::GATT_CONTROL_UUID',
  'denzic_ota_core::GATT_DATA_UUID',
  'denzic_ota_core::GATT_STATUS_UUID',
]) {
  assert.ok(
    rustFirmwareOtaSource.includes(expected),
    `Listener OTA Rust manifest validation must use shared ${expected}`,
  );
}
for (const uuid of [
  LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.serviceUuid,
  LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.controlUuid,
  LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.dataUuid,
  LISTENER_OTA_V1_TRANSPORT_BOUNDARY.gatt.statusUuid,
]) {
  assert.ok(
    !rustFirmwareOtaSource.includes(uuid),
    `Listener OTA UUID ${uuid} must not be duplicated in src-tauri/src/firmware_ota.rs`,
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
  firmwareOtaPanelSource.includes('const OTA_PREFLIGHT_SNAPSHOT_FRESH_MS = 60_000;'),
  'OTA start should reuse the package-selection preflight long enough for a normal user to review it before starting',
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
  firmwareOtaPanelSource.includes('className="ol-firmware-selected-package"'),
  'selected firmware package must remain visibly pinned after selection',
);
assert.ok(
  /<button[\s\S]*?className="ol-firmware-selected-package"[\s\S]*?choosePackage\(selectedPackage\.sourceKind === 'directory'\)/.test(firmwareOtaPanelSource),
  'selected firmware package must replace the picker with a clickable package control that can reselect the same source kind',
);
assert.ok(
  firmwareOtaPanelSource.includes("sourceKind: 'zip' | 'directory'"),
  'selected firmware package must remember whether the chosen package came from a zip or directory picker',
);
assert.ok(
  firmwareOtaPanelSource.indexOf('className="ol-firmware-selected-package"') <
    firmwareOtaPanelSource.indexOf("{firmwareMode === 'ble' && ("),
  'selected firmware package summary must be outside the BLE/wired mode-specific panels',
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
assert.ok(
  firmwareOtaPanelSource.includes('firmwareOtaSnapshotSatisfiesVersionRefreshFallback(snapshot, protocolName)'),
  'manual Denzic OTA v1 refresh must stop polling when the service is reachable even if DIS firmware version is unavailable',
);
assert.ok(
  !firmwareOtaPanelSource.includes("protocolName === 'listener_ota_v1'"),
  'manual Listener OTA v1 refresh must not use stale protocol-name strings',
);

const commandsSource = readFileSync('src-tauri/src/commands.rs', 'utf8');
assert.ok(
  commandsSource.includes('FIRMWARE_OTA_LISTENER_V1_PREFLIGHT_TIMEOUT'),
  'firmware OTA preflight IPC must keep an outer timeout around Windows BLE snapshot probing',
);
assert.ok(
  commandsSource.includes('tokio::time::timeout(\n        FIRMWARE_OTA_LISTENER_V1_PREFLIGHT_TIMEOUT,\n        snapshot_task,'),
  'firmware OTA preflight IPC timeout must wrap the blocking BLE snapshot task',
);
assert.ok(
  commandsSource.includes('protocol_name: Option<String>'),
  'firmware OTA preflight IPC must accept the selected package protocol',
);
assert.ok(
  commandsSource.includes('FIRMWARE_OTA_LISTENER_V1_GATT_PROBE_TIMEOUT')
    && commandsSource.includes('FIRMWARE_OTA_LISTENER_V1_PREFLIGHT_TIMEOUT')
    && commandsSource.includes('FIRMWARE_OTA_LISTENER_V1_GATT_PROBE_TIMEOUT: Duration = Duration::from_secs(8)')
    && commandsSource.includes('FIRMWARE_OTA_LISTENER_V1_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10)')
    && commandsSource.includes('crate::embedded_ble::listener_ota_v1_gatt_probe_snapshot(')
    && commandsSource.includes('coord.embedded_ble_wake_recovery_snapshot()'),
  'Denzic OTA v1 preflight must keep enough bounded time for a post-reconnect Windows GATT service query and merge the cached power state',
);
assert.ok(
  commandsSource.includes('confirm_listener_ota_v1_reachable(&version).await'),
  'Listener OTA v1 UI transfer confirmation must use the fast reachable-service confirmation path',
);
assert.ok(
  commandsSource.includes('tauri::async_runtime::spawn_blocking(|| {\n            crate::embedded_ble::listener_ota_v1_gatt_probe_snapshot('),
  'Listener OTA v1 confirmation must probe only the OTA service instead of waiting for optional DIS metadata',
);
assert.ok(
  commandsSource.includes('FIRMWARE_OTA_LISTENER_V1_REACHABLE_CONFIRM_TIMEOUT'),
  'Listener OTA v1 UI transfer confirmation must have a bounded short timeout separate from version polling',
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
for (const removedSlowFallback of ['listener_ota_v1_snapshot_with_identity_fallback', 'merge_listener_ota_v1_snapshot_identity']) {
  assert.ok(
    !rustFirmwareOtaSource.includes(removedSlowFallback),
    `Listener OTA v1 preflight must not restore slow stable-anchor identity fallback: ${removedSlowFallback}`,
  );
}
assert.ok(
  rustFirmwareOtaSource.includes('listener_ota_v1_preflight_allows_reachable_device_without_identity_metadata'),
  'Denzic OTA v1 preflight must allow a reachable service when Windows omits optional identity metadata',
);
assert.ok(
  rustFirmwareOtaSource.includes('confirm_listener_ota_v1_reachable_version(&expected_version).await'),
  'Listener OTA v1 headless transfer confirmation must use the fast reachable-service confirmation path',
);
assert.ok(
  rustFirmwareOtaSource.includes('LISTENER_OTA_V1_REACHABLE_CONFIRM_TIMEOUT'),
  'Listener OTA v1 headless transfer confirmation must have a bounded short timeout separate from version polling',
);

const embeddedBleSource = readFileSync('src-tauri/src/embedded_ble.rs', 'utf8');
assert.ok(
  embeddedBleSource.includes('LISTENER_OTA_V1_DEFAULT_WINDOW_CHUNKS: usize = 100'),
  'Listener OTA v1 must keep the measured 100-chunk default needed for the under-60-second UI transfer target',
);
for (const expected of [
  'denzic_ota_core::GATT_SERVICE_UUID_U128',
  'denzic_ota_core::GATT_CONTROL_UUID_U128',
  'denzic_ota_core::GATT_DATA_UUID_U128',
  'denzic_ota_core::GATT_STATUS_UUID_U128',
]) {
  assert.ok(
    embeddedBleSource.includes(expected),
    `Listener OTA WinRT adapter must use shared ${expected}`,
  );
}
const listenerOtaV1SnapshotStart = embeddedBleSource.indexOf('fn listener_ota_v1_device_snapshot_from_target');
const listenerOtaV1SnapshotEnd = embeddedBleSource.indexOf('fn listener_ota_v1_gatt_probe_snapshot_from_target', listenerOtaV1SnapshotStart);
assert.ok(listenerOtaV1SnapshotStart >= 0 && listenerOtaV1SnapshotEnd > listenerOtaV1SnapshotStart);
const listenerOtaV1SnapshotBody = embeddedBleSource.slice(listenerOtaV1SnapshotStart, listenerOtaV1SnapshotEnd);
assert.ok(
  embeddedBleSource.includes('DIS metadata is best-effort'),
  'Listener OTA v1 snapshot must document that DIS metadata is best-effort and not a hard blocker',
);
assert.ok(
  embeddedBleSource.includes('for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached]'),
  'Listener OTA v1 discovery must prefer uncached characteristics so a firmware GATT schema update cannot reuse stale handles',
);
assert.ok(
  !listenerOtaV1SnapshotBody.includes('read_optional_string_characteristic_from_service'),
  'Listener OTA v1 snapshot must not probe optional readiness/capability characteristics before transfer',
);
assert.ok(
  listenerOtaV1SnapshotBody.includes('read_dis_metadata_from_discovered_services(target.bluetooth_address)'),
  'Listener OTA v1 snapshot should fill hardware/firmware metadata from bounded DIS discovery when Windows exposes it',
);

const ipcSource = readFileSync('src/lib/ipc.ts', 'utf8');
for (const expectedTimingField of ['transferElapsedMs', 'confirmElapsedMs', 'totalElapsedMs']) {
  assert.ok(
    ipcSource.includes(expectedTimingField),
    `firmware OTA IPC type must expose ${expectedTimingField}`,
  );
}
