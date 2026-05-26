export type FirmwareOtaChannel = 'stable' | 'beta' | 'internal-test';

export type FirmwareOtaUserState =
  | 'idle'
  | 'checking'
  | 'ready'
  | 'transferring'
  | 'rebooting'
  | 'verifying'
  | 'success'
  | 'failed'
  | 'rolledBack';

export type FirmwareOtaBlockerCode =
  | 'deviceDisconnected'
  | 'recordingActive'
  | 'transferActive'
  | 'deviceStatusUnknown'
  | 'batteryLow'
  | 'powerUnknown'
  | 'hardwareMismatch'
  | 'missingCapability'
  | 'minDesktopVersion'
  | 'sameVersion';

export type FirmwareOtaFailureCode =
  | 'bleDisconnected'
  | 'manifestMismatch'
  | 'hashFailure'
  | 'deviceRejected'
  | 'versionNotConfirmed'
  | 'rolledBack';

export interface FirmwareOtaManifest {
  schemaVersion: number;
  packageType: 'listener-firmware-ota';
  project: string;
  version: string;
  protocolName: string;
  protocolVersion: number;
  hardwareRevision: string;
  minDesktopVersion: string;
  channel: FirmwareOtaChannel;
  fileName: string;
  fileSizeBytes: number;
  fileSha256: string;
  firmwareCapability: string;
  gattServiceUuid: string;
  gattControlUuid: string;
  gattDataUuid: string;
  gattChunkBytes: number;
  rollbackInstructions: string[];
  recoveryInstructions: string[];
}

export interface FirmwareOtaValidationContext {
  desktopVersion: string;
  expectedHardwareRevision: string;
  currentFirmwareVersion?: string | null;
}

export interface FirmwareOtaValidationResult {
  ok: boolean;
  manifest: FirmwareOtaManifest | null;
  firmwareSha256: string | null;
  errors: string[];
  warnings: string[];
}

export interface FirmwareOtaDeviceSnapshot {
  connected: boolean;
  hardwareRevision: string | null;
  firmwareVersion: string | null;
  capabilities: string[];
  batteryPercent?: number | null;
  usbPowered?: boolean | null;
  detail?: string | null;
}

export interface FirmwareOtaPreflightSnapshot {
  recordingActive: boolean;
  dictationPhase: string;
  device: FirmwareOtaDeviceSnapshot;
}

export interface FirmwareOtaPreflightInput {
  manifest: FirmwareOtaManifest;
  device: FirmwareOtaDeviceSnapshot;
  desktopVersion: string;
  recordingActive: boolean;
  transferActive: boolean;
}

export interface FirmwareOtaBlocker {
  code: FirmwareOtaBlockerCode;
  message: string;
  nextStep: string;
}

export interface FirmwareOtaPreflightResult {
  ok: boolean;
  blockers: FirmwareOtaBlocker[];
}

export interface FirmwareOtaState {
  userState: FirmwareOtaUserState;
  progress: number;
  message: string;
  failureCode: FirmwareOtaFailureCode | null;
}

export type FirmwareOtaAction =
  | { type: 'check' }
  | { type: 'ready' }
  | { type: 'startTransfer' }
  | { type: 'transferProgress'; progress: number }
  | { type: 'transferComplete' }
  | { type: 'deviceReconnected' }
  | { type: 'verified' }
  | { type: 'failed'; failureCode: FirmwareOtaFailureCode; message: string }
  | { type: 'rolledBack'; message: string }
  | { type: 'retry' };

export const FIRMWARE_OTA_REQUIRED_PUBLIC_STATES: readonly FirmwareOtaUserState[] = [
  'checking',
  'ready',
  'transferring',
  'rebooting',
  'verifying',
  'success',
  'failed',
  'rolledBack',
] as const;

export const FIRMWARE_OTA_TRANSPORT_BOUNDARY = {
  protocolName: 'listener_ble_ota',
  firmwareCapability: 'firmware_ota_v1',
  gatt: {
    serviceUuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092a',
    controlUuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092b',
    dataUuid: '710af845-6d9f-6583-0c4d-9e5b3bc3092c',
    chunkBytes: 180,
  },
  dataPlane: 'dedicated OTA GATT service',
  notDataPlane: ['BLE audio VKA1 notifications', 'BLE HID keyboard reports'],
} as const;

const SHA256_RE = /^[0-9a-f]{64}$/;
const MIN_BATTERY_PERCENT = 20;

export async function validateFirmwareOtaPackage(
  manifestText: string,
  firmwareBytes: Uint8Array,
  context: FirmwareOtaValidationContext,
): Promise<FirmwareOtaValidationResult> {
  const errors: string[] = [];
  const warnings: string[] = [];
  let manifest: FirmwareOtaManifest | null = null;

  try {
    manifest = parseFirmwareOtaManifest(JSON.parse(manifestText));
  } catch (error) {
    errors.push(error instanceof Error ? error.message : String(error));
    return { ok: false, manifest: null, firmwareSha256: null, errors, warnings };
  }

  if (manifest.fileName !== 'firmware_ota.bin') {
    errors.push('Package must include firmware_ota.bin.');
  }
  if (firmwareBytes.byteLength !== manifest.fileSizeBytes) {
    errors.push(`Firmware size mismatch: manifest=${manifest.fileSizeBytes}, actual=${firmwareBytes.byteLength}.`);
  }

  const firmwareSha256 = await sha256Hex(firmwareBytes);
  if (firmwareSha256 !== manifest.fileSha256) {
    errors.push('Firmware SHA256 does not match ota_manifest.json.');
  }
  if (!versionsCompatible(context.desktopVersion, manifest.minDesktopVersion)) {
    errors.push(`Listener Type ${context.desktopVersion} is older than required ${manifest.minDesktopVersion}.`);
  }
  if (manifest.hardwareRevision !== context.expectedHardwareRevision) {
    errors.push(`Hardware revision mismatch: package=${manifest.hardwareRevision}, expected=${context.expectedHardwareRevision}.`);
  }
  if (context.currentFirmwareVersion && compareVersionish(manifest.version, context.currentFirmwareVersion) <= 0) {
    warnings.push('Package version is not newer than the connected firmware version.');
  }

  return {
    ok: errors.length === 0,
    manifest,
    firmwareSha256,
    errors,
    warnings,
  };
}

export function parseFirmwareOtaManifest(value: unknown): FirmwareOtaManifest {
  if (!isRecord(value)) {
    throw new Error('ota_manifest.json must be a JSON object.');
  }
  const schemaVersion = requireNumber(value.schema_version ?? value.schemaVersion, 'schema_version');
  if (schemaVersion === 1) return parseFirmwareOtaManifestV1(value, schemaVersion);
  if (schemaVersion === 2) return parseFirmwareOtaManifestV2(value, schemaVersion);
  throw new Error(`Unsupported OTA manifest schema_version ${schemaVersion}.`);
}

function parseFirmwareOtaManifestV1(value: Record<string, unknown>, schemaVersion: number): FirmwareOtaManifest {
  const file = requireRecord(value.file, 'file');
  const protocol = requireRecord(value.protocol, 'protocol');
  const rollback = requireRecord(value.rollback, 'rollback');
  const recovery = requireRecord(value.recovery, 'recovery');
  const gatt = isRecord(protocol.gatt) ? protocol.gatt : null;

  const manifest: FirmwareOtaManifest = {
    schemaVersion,
    packageType: requireString(value.package_type ?? value.packageType, 'package_type') as 'listener-firmware-ota',
    project: requireString(value.project, 'project'),
    version: requireString(value.version, 'version'),
    protocolName: requireString(protocol.name, 'protocol.name'),
    protocolVersion: requireNumber(protocol.version, 'protocol.version'),
    hardwareRevision: requireString(value.hardware_revision ?? value.hardwareRevision, 'hardware_revision'),
    minDesktopVersion: requireString(value.min_desktop_version ?? value.minDesktopVersion, 'min_desktop_version'),
    channel: requireChannel(value.channel),
    fileName: requireString(file.name, 'file.name'),
    fileSizeBytes: requireNumber(file.size_bytes ?? file.sizeBytes, 'file.size_bytes'),
    fileSha256: requireString(file.sha256, 'file.sha256').toLowerCase(),
    firmwareCapability: requireString(protocol.firmware_capability ?? protocol.firmwareCapability, 'protocol.firmware_capability'),
    gattServiceUuid: gatt
      ? requireString(gatt.service_uuid ?? gatt.serviceUuid, 'protocol.gatt.service_uuid')
      : FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.serviceUuid,
    gattControlUuid: gatt
      ? requireString(gatt.control_uuid ?? gatt.controlUuid, 'protocol.gatt.control_uuid')
      : FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.controlUuid,
    gattDataUuid: gatt
      ? requireString(gatt.data_uuid ?? gatt.dataUuid, 'protocol.gatt.data_uuid')
      : FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.dataUuid,
    gattChunkBytes: gatt
      ? requireNumber(gatt.chunk_bytes ?? gatt.chunkBytes, 'protocol.gatt.chunk_bytes')
      : FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.chunkBytes,
    rollbackInstructions: requireInstructions(rollback.instructions, 'rollback.instructions'),
    recoveryInstructions: requireInstructions(recovery.instructions, 'recovery.instructions'),
  };

  validateNormalizedFirmwareOtaManifest(manifest);
  return manifest;
}

function parseFirmwareOtaManifestV2(value: Record<string, unknown>, schemaVersion: number): FirmwareOtaManifest {
  const firmware = requireRecord(value.firmware, 'firmware');
  const requirements = requireRecord(value.requirements, 'requirements');
  const bleIdentity = requireRecord(value.ble_identity ?? value.bleIdentity, 'ble_identity');
  const dis = requireRecord(bleIdentity.dis, 'ble_identity.dis');
  const rollback = requireRecord(value.rollback, 'rollback');
  const recovery = requireRecord(value.recovery, 'recovery');
  const rollbackSupported = requireBool(rollback.supported, 'rollback.supported');
  if (!rollbackSupported) {
    throw new Error('rollback.supported must be true.');
  }

  requireString(value.created_at_utc ?? value.createdAtUtc, 'created_at_utc');
  requireString(firmware.git_commit ?? firmware.gitCommit, 'firmware.git_commit');
  requireBool(firmware.git_dirty ?? firmware.gitDirty, 'firmware.git_dirty');
  requireString(firmware.target, 'firmware.target');
  requireString(bleIdentity.name, 'ble_identity.name');
  requireString(bleIdentity.appearance, 'ble_identity.appearance');
  requireString(dis.model, 'ble_identity.dis.model');
  requireString(dis.hardware_revision ?? dis.hardwareRevision, 'ble_identity.dis.hardware_revision');
  requireString(dis.firmware_revision ?? dis.firmwareRevision, 'ble_identity.dis.firmware_revision');

  const rollbackMethod = requireString(rollback.method, 'rollback.method');
  if (rollbackMethod !== 'esp_idf_bootloader_rollback') {
    throw new Error(`Unsupported rollback.method ${rollbackMethod}.`);
  }
  const factoryReflash = requireString(recovery.factory_reflash ?? recovery.factoryReflash, 'recovery.factory_reflash');
  const serialCommands = requireString(recovery.serial_commands ?? recovery.serialCommands, 'recovery.serial_commands');

  const manifest: FirmwareOtaManifest = {
    schemaVersion,
    packageType: 'listener-firmware-ota',
    project: requireString(firmware.project, 'firmware.project'),
    version: requireString(firmware.version, 'firmware.version'),
    protocolName: FIRMWARE_OTA_TRANSPORT_BOUNDARY.protocolName,
    protocolVersion: requireNumber(requirements.protocol_version ?? requirements.protocolVersion, 'requirements.protocol_version'),
    hardwareRevision: requireString(requirements.hardware_revision ?? requirements.hardwareRevision, 'requirements.hardware_revision'),
    minDesktopVersion: requireString(requirements.min_desktop_version ?? requirements.minDesktopVersion, 'requirements.min_desktop_version'),
    channel: requireChannel(value.channel),
    fileName: requireString(firmware.file, 'firmware.file'),
    fileSizeBytes: requireNumber(firmware.size_bytes ?? firmware.sizeBytes, 'firmware.size_bytes'),
    fileSha256: requireString(firmware.sha256, 'firmware.sha256').toLowerCase(),
    firmwareCapability: FIRMWARE_OTA_TRANSPORT_BOUNDARY.firmwareCapability,
    gattServiceUuid: FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.serviceUuid,
    gattControlUuid: FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.controlUuid,
    gattDataUuid: FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.dataUuid,
    gattChunkBytes: FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.chunkBytes,
    rollbackInstructions: requireInstructions(rollback.instructions, 'rollback.instructions'),
    recoveryInstructions: [factoryReflash, serialCommands],
  };

  validateNormalizedFirmwareOtaManifest(manifest);
  return manifest;
}

function validateNormalizedFirmwareOtaManifest(manifest: FirmwareOtaManifest): void {
  if (manifest.packageType !== 'listener-firmware-ota') {
    throw new Error('ota_manifest.json package_type must be listener-firmware-ota.');
  }
  if (manifest.protocolName !== FIRMWARE_OTA_TRANSPORT_BOUNDARY.protocolName) {
    throw new Error(`Unsupported OTA protocol ${manifest.protocolName}.`);
  }
  if (manifest.protocolVersion < 1) {
    throw new Error('OTA protocol.version must be >= 1.');
  }
  if (manifest.firmwareCapability !== FIRMWARE_OTA_TRANSPORT_BOUNDARY.firmwareCapability) {
    throw new Error('OTA package requires unsupported firmware capability.');
  }
  if (
    manifest.gattServiceUuid !== FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.serviceUuid ||
    manifest.gattControlUuid !== FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.controlUuid ||
    manifest.gattDataUuid !== FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.dataUuid ||
    manifest.gattChunkBytes !== FIRMWARE_OTA_TRANSPORT_BOUNDARY.gatt.chunkBytes
  ) {
    throw new Error('OTA package uses an unsupported BLE OTA GATT boundary.');
  }
  if (manifest.fileSizeBytes <= 0) {
    throw new Error('file.size_bytes must be greater than zero.');
  }
  if (!SHA256_RE.test(manifest.fileSha256)) {
    throw new Error('file.sha256 must be lowercase SHA256 hex.');
  }
}

export function evaluateFirmwareOtaPreflight(input: FirmwareOtaPreflightInput): FirmwareOtaPreflightResult {
  const blockers: FirmwareOtaBlocker[] = [];
  const { manifest, device } = input;

  if (!device.connected) {
    blockers.push(blocker(
      'deviceDisconnected',
      device.detail ? `Device is not ready for OTA: ${device.detail}` : 'Device is not connected.',
      'Connect Listener in Windows Bluetooth, then refresh Listener BLE status.',
    ));
  }
  if (input.recordingActive) {
    blockers.push(blocker(
      'recordingActive',
      'Recording is still active.',
      'Stop or cancel the current recording before updating firmware.',
    ));
  }
  if (input.transferActive) {
    blockers.push(blocker(
      'transferActive',
      'Another transfer is in progress.',
      'Wait for the current transfer to finish, or retry after it fails.',
    ));
  }
  if (!versionsCompatible(input.desktopVersion, manifest.minDesktopVersion)) {
    blockers.push(blocker(
      'minDesktopVersion',
      'Listener Type is too old for this firmware package.',
      `Update Listener Type to ${manifest.minDesktopVersion} or newer first.`,
    ));
  }
  if (device.connected && !device.hardwareRevision) {
    blockers.push(blocker(
      'deviceStatusUnknown',
      'Device hardware revision is unknown.',
      'Refresh Listener BLE status; if it remains unknown, use the USB factory package.',
    ));
  } else if (device.hardwareRevision && device.hardwareRevision !== manifest.hardwareRevision) {
    blockers.push(blocker(
      'hardwareMismatch',
      'Firmware package is for a different hardware revision.',
      'Use an OTA package built for this Listener device.',
    ));
  }
  if (!device.capabilities.includes(manifest.firmwareCapability)) {
    blockers.push(blocker(
      'missingCapability',
      'Connected firmware does not advertise OTA support.',
      'Use the USB factory package once, then retry OTA from Listener Type.',
    ));
  }
  if (device.firmwareVersion && compareVersionish(manifest.version, device.firmwareVersion) <= 0) {
    blockers.push(blocker(
      'sameVersion',
      'This package is not newer than the device firmware.',
      'Choose a newer firmware package.',
    ));
  }

  const battery = device.batteryPercent;
  if (device.usbPowered === false && typeof battery === 'number' && battery < MIN_BATTERY_PERCENT) {
    blockers.push(blocker(
      'batteryLow',
      'Battery is too low for firmware update.',
      'Connect USB power or charge the device above 20%.',
    ));
  } else if (device.usbPowered !== true && battery == null) {
    blockers.push(blocker(
      'powerUnknown',
      'Power state is unknown.',
      'Connect USB power before starting the update.',
    ));
  }

  return { ok: blockers.length === 0, blockers };
}

export const initialFirmwareOtaState: FirmwareOtaState = {
  userState: 'idle',
  progress: 0,
  message: '',
  failureCode: null,
};

export function firmwareOtaReducer(state: FirmwareOtaState, action: FirmwareOtaAction): FirmwareOtaState {
  switch (action.type) {
    case 'check':
      return { userState: 'checking', progress: 0, message: 'Checking package', failureCode: null };
    case 'ready':
      return { userState: 'ready', progress: 0, message: 'Ready to update', failureCode: null };
    case 'startTransfer':
      return { userState: 'transferring', progress: Math.max(state.progress, 1), message: 'Transferring firmware', failureCode: null };
    case 'transferProgress':
      return { ...state, userState: 'transferring', progress: clamp(action.progress, 1, 99), message: 'Transferring firmware' };
    case 'transferComplete':
      return { userState: 'rebooting', progress: 100, message: 'Rebooting device', failureCode: null };
    case 'deviceReconnected':
      return { userState: 'verifying', progress: 100, message: 'Verifying firmware version', failureCode: null };
    case 'verified':
      return { userState: 'success', progress: 100, message: 'Firmware updated', failureCode: null };
    case 'failed':
      return { userState: 'failed', progress: state.progress, message: action.message, failureCode: action.failureCode };
    case 'rolledBack':
      return { userState: 'rolledBack', progress: state.progress, message: action.message, failureCode: 'rolledBack' };
    case 'retry':
      return { userState: 'checking', progress: 0, message: 'Checking package', failureCode: null };
  }
}

export function firmwareOtaFailureNextStep(code: FirmwareOtaFailureCode): string {
  switch (code) {
    case 'bleDisconnected':
      return 'Reconnect Listener in Windows Bluetooth, refresh Listener BLE, then retry.';
    case 'manifestMismatch':
      return 'Choose the ota_manifest.json and firmware_ota.bin generated in the same package directory.';
    case 'hashFailure':
      return 'Download or rebuild the OTA package again; do not retry a package with a bad hash.';
    case 'deviceRejected':
      return 'Export diagnostics, check the device blocker, then retry after the blocker is clear.';
    case 'versionNotConfirmed':
      return 'Wait for the device to reconnect. If the version is still unchanged, retry or export diagnostics.';
    case 'rolledBack':
      return 'The device returned to the previous firmware. Export diagnostics before retrying.';
  }
}

export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  if (!globalThis.crypto?.subtle) {
    throw new Error('SHA256 validation requires Web Crypto.');
  }
  const source = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(source).set(bytes);
  const digest = await globalThis.crypto.subtle.digest('SHA-256', source);
  return Array.from(new Uint8Array(digest))
    .map(byte => byte.toString(16).padStart(2, '0'))
    .join('');
}

function blocker(code: FirmwareOtaBlockerCode, message: string, nextStep: string): FirmwareOtaBlocker {
  return { code, message, nextStep };
}

function versionsCompatible(current: string, minimum: string): boolean {
  return compareVersionish(current, minimum) >= 0;
}

export function compareVersionish(left: string, right: string): number {
  const a = versionParts(left);
  const b = versionParts(right);
  const len = Math.max(a.length, b.length);
  for (let i = 0; i < len; i += 1) {
    const delta = (a[i] ?? 0) - (b[i] ?? 0);
    if (delta !== 0) return delta > 0 ? 1 : -1;
  }
  return 0;
}

function versionParts(value: string): number[] {
  const match = value.trim().replace(/^v/i, '').match(/\d+(?:\.\d+)*/);
  if (!match) return [0];
  return match[0].split('.').map(part => Number.parseInt(part, 10)).filter(Number.isFinite);
}

function requireRecord(value: unknown, field: string): Record<string, unknown> {
  if (!isRecord(value)) {
    throw new Error(`${field} must be an object.`);
  }
  return value;
}

function requireString(value: unknown, field: string): string {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new Error(`${field} must be a non-empty string.`);
  }
  return value.trim();
}

function requireNumber(value: unknown, field: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new Error(`${field} must be a number.`);
  }
  return value;
}

function requireBool(value: unknown, field: string): boolean {
  if (typeof value !== 'boolean') {
    throw new Error(`${field} must be a boolean.`);
  }
  return value;
}

function requireChannel(value: unknown): FirmwareOtaChannel {
  if (value === 'stable' || value === 'beta' || value === 'internal-test') {
    return value;
  }
  throw new Error('channel must be stable, beta, or internal-test.');
}

function requireInstructions(value: unknown, field: string): string[] {
  if (typeof value === 'string' && value.trim() !== '') {
    return [value.trim()];
  }
  if (!Array.isArray(value)) {
    throw new Error(`${field} must be an array.`);
  }
  const strings = value.filter(item => typeof item === 'string' && item.trim() !== '').map(item => item.trim());
  if (strings.length === 0) {
    throw new Error(`${field} must contain at least one instruction.`);
  }
  return strings;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
