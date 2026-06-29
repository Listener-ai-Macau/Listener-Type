export const COMPANION_V1_SCHEMA = 'companion.host.v1' as const;
export const COMPANION_V1_BLE_NAME_MAX_BYTES = 29;

export const COMPANION_V1_GATT_BOUNDARY = {
  settingsServiceUuid: '8f7a0004-7b7d-4f3d-9d6f-6c2d1b7c0000',
  bleNameConfigUuid: '8f7a4002-7b7d-4f3d-9d6f-6c2d1b7c0000',
  meetingServiceUuid: '8f7a0008-7b7d-4f3d-9d6f-6c2d1b7c0000',
  meetingControlUuid: '8f7a8001-7b7d-4f3d-9d6f-6c2d1b7c0000',
  meetingStatusUuid: '8f7a8002-7b7d-4f3d-9d6f-6c2d1b7c0000',
  sensorsServiceUuid: '8f7a0009-7b7d-4f3d-9d6f-6c2d1b7c0000',
  imuStatusUuid: '8f7a9001-7b7d-4f3d-9d6f-6c2d1b7c0000',
  audioOutputServiceUuid: '8f7a000a-7b7d-4f3d-9d6f-6c2d1b7c0000',
  speakerControlUuid: '8f7aa001-7b7d-4f3d-9d6f-6c2d1b7c0000',
  speakerStatusUuid: '8f7aa002-7b7d-4f3d-9d6f-6c2d1b7c0000',
  wakeWordControlUuid: '8f7aa003-7b7d-4f3d-9d6f-6c2d1b7c0000',
  wakeWordStatusUuid: '8f7aa004-7b7d-4f3d-9d6f-6c2d1b7c0000',
} as const;

export type CompanionV1SnapshotSource = 'fixture' | 'firmware' | 'lastKnown' | 'unavailable';

export type CompanionV1ControlAction =
  | 'stageBleName'
  | 'applyBleName'
  | 'resetBleName'
  | 'meetingStart'
  | 'meetingStop'
  | 'meetingFinalize'
  | 'speakerPrompt'
  | 'speakerStop'
  | 'wakeArm'
  | 'wakeDisable'
  | 'wakeTestDetect';

export interface CompanionV1MeetingState {
  state: 'idle' | 'recording' | 'stored' | 'finalized' | string;
  meetingId: number;
  segmentCount: number;
  durationMs: number;
  summaryState: 'notStarted' | 'pendingHostSummary' | string;
  syncState: 'none' | 'recording' | 'readyForHostSync' | string;
}

export interface CompanionV1ImuState {
  sensorState: 'notPopulatedOnCurrentDevBoard' | 'ready' | 'error' | string;
  motionFlags: string[];
  sampleRateHz: number;
  calibrationState: 'pendingHardware' | 'calibrated' | string;
  sampleTimestampMs: number;
  accelMg: [number, number, number];
  gyroDps: [number, number, number];
  temperatureCX10: number;
}

export interface CompanionV1SpeakerState {
  state: 'promptReadyPendingHardware' | 'promptQueuedFixture' | 'idle' | string;
  promptId: number;
  volumePercent: number;
  playedCount: number;
  lastError: string;
}

export interface CompanionV1WakeWordState {
  armed: boolean;
  engineState: 'armedFixture' | 'disabledFixture' | 'detectedFixture' | string;
  sensitivity: number;
  lastEvent: 'none' | 'armed' | 'disabled' | 'detected' | string;
  lastConfidence: number;
  detectionCount: number;
  rejectedCount: number;
}

export interface CompanionV1BleNameState {
  activeName: string;
  stagedName: string;
  maxLen: number;
  pendingRestart: boolean;
  status: 'active' | 'staged' | 'restartRequired' | 'rejected' | string;
  policy: string;
}

export interface CompanionV1ControlEcho {
  action: CompanionV1ControlAction;
  characteristicUuid: string;
  payloadHex: string;
  transport: 'fixtureNoHardware' | 'bleGatt' | string;
}

export interface CompanionV1Snapshot {
  schema: typeof COMPANION_V1_SCHEMA;
  source: CompanionV1SnapshotSource;
  connected: boolean;
  writeSupported: boolean;
  detail: string | null;
  gatt: typeof COMPANION_V1_GATT_BOUNDARY;
  meeting: CompanionV1MeetingState;
  imu: CompanionV1ImuState;
  speaker: CompanionV1SpeakerState;
  wakeWord: CompanionV1WakeWordState;
  bleName: CompanionV1BleNameState;
  lastControl: CompanionV1ControlEcho | null;
}

export interface CompanionV1ControlRequest {
  action: CompanionV1ControlAction;
  requestId?: number;
  bleName?: string;
  maxSeconds?: number;
  profileId?: number;
  volumePercent?: number;
  promptId?: number;
  sensitivity?: number;
}

export function companionBleNameIsValid(value: string): boolean {
  const bytes = new TextEncoder().encode(value);
  if (bytes.length < 1 || bytes.length > COMPANION_V1_BLE_NAME_MAX_BYTES) return false;
  for (const byte of bytes) {
    if (byte < 0x20 || byte > 0x7e) return false;
    if (byte === 0x22 || byte === 0x27 || byte === 0x3b || byte === 0x3d || byte === 0x5c) {
      return false;
    }
  }
  return true;
}

export function createCompanionV1Fixture(
  overrides: Partial<CompanionV1Snapshot> = {},
): CompanionV1Snapshot {
  return {
    schema: COMPANION_V1_SCHEMA,
    source: 'fixture',
    connected: false,
    writeSupported: true,
    detail: 'No Companion hardware is online; browser preview uses the firmware V1 fixture.',
    gatt: COMPANION_V1_GATT_BOUNDARY,
    meeting: {
      state: 'idle',
      meetingId: 0,
      segmentCount: 0,
      durationMs: 0,
      summaryState: 'notStarted',
      syncState: 'none',
    },
    imu: {
      sensorState: 'notPopulatedOnCurrentDevBoard',
      motionFlags: [],
      sampleRateHz: 0,
      calibrationState: 'pendingHardware',
      sampleTimestampMs: 0,
      accelMg: [0, 0, 0],
      gyroDps: [0, 0, 0],
      temperatureCX10: 0,
    },
    speaker: {
      state: 'promptReadyPendingHardware',
      promptId: 7,
      volumePercent: 50,
      playedCount: 1,
      lastError: 'none',
    },
    wakeWord: {
      armed: true,
      engineState: 'armedFixture',
      sensitivity: 50,
      lastEvent: 'none',
      lastConfidence: 0,
      detectionCount: 0,
      rejectedCount: 0,
    },
    bleName: {
      activeName: 'companion',
      stagedName: 'companion',
      maxLen: COMPANION_V1_BLE_NAME_MAX_BYTES,
      pendingRestart: false,
      status: 'active',
      policy: 'printableAscii1To29NoQuoteSemicolonEqualsBackslash',
    },
    lastControl: null,
    ...overrides,
  };
}

export function applyCompanionV1ControlFixture(
  snapshot: CompanionV1Snapshot,
  request: CompanionV1ControlRequest,
): CompanionV1Snapshot {
  const next = structuredClone(snapshot) as CompanionV1Snapshot;
  const characteristicUuid = characteristicForAction(request.action);
  next.lastControl = {
    action: request.action,
    characteristicUuid,
    payloadHex: fixturePayloadHex(request),
    transport: 'fixtureNoHardware',
  };
  next.source = 'fixture';

  switch (request.action) {
    case 'stageBleName': {
      const name = request.bleName ?? '';
      if (!companionBleNameIsValid(name)) {
        throw new Error('invalidCompanionBleName');
      }
      next.bleName = {
        ...next.bleName,
        stagedName: name,
        status: 'staged',
        pendingRestart: true,
      };
      break;
    }
    case 'applyBleName':
      next.bleName = { ...next.bleName, status: 'restartRequired', pendingRestart: true };
      break;
    case 'resetBleName':
      next.bleName = { ...next.bleName, stagedName: 'companion', status: 'staged', pendingRestart: true };
      break;
    case 'meetingStart':
      next.meeting = { ...next.meeting, state: 'recording', meetingId: 1, segmentCount: 0, syncState: 'recording' };
      break;
    case 'meetingStop':
      next.meeting = {
        ...next.meeting,
        state: 'stored',
        meetingId: 1,
        segmentCount: 1,
        durationMs: 2400,
        summaryState: 'pendingHostSummary',
        syncState: 'readyForHostSync',
      };
      break;
    case 'meetingFinalize':
      next.meeting = {
        ...next.meeting,
        state: 'finalized',
        meetingId: 1,
        segmentCount: 1,
        durationMs: 2400,
        summaryState: 'pendingHostSummary',
        syncState: 'readyForHostSync',
      };
      break;
    case 'speakerPrompt':
      next.speaker = {
        ...next.speaker,
        state: 'promptQueuedFixture',
        promptId: request.promptId ?? next.speaker.promptId,
        volumePercent: Math.min(100, request.volumePercent ?? next.speaker.volumePercent),
        playedCount: next.speaker.playedCount + 1,
      };
      break;
    case 'speakerStop':
      next.speaker = { ...next.speaker, state: 'idle' };
      break;
    case 'wakeArm':
      next.wakeWord = {
        ...next.wakeWord,
        armed: true,
        engineState: 'armedFixture',
        sensitivity: Math.min(100, request.sensitivity ?? next.wakeWord.sensitivity),
        lastEvent: 'armed',
      };
      break;
    case 'wakeDisable':
      next.wakeWord = { ...next.wakeWord, armed: false, engineState: 'disabledFixture', lastEvent: 'disabled' };
      break;
    case 'wakeTestDetect':
      next.wakeWord = {
        ...next.wakeWord,
        armed: true,
        engineState: 'detectedFixture',
        lastEvent: 'detected',
        lastConfidence: 88,
        detectionCount: next.wakeWord.detectionCount + 1,
      };
      break;
  }
  return next;
}

function characteristicForAction(action: CompanionV1ControlAction): string {
  if (action === 'stageBleName' || action === 'applyBleName' || action === 'resetBleName') {
    return COMPANION_V1_GATT_BOUNDARY.bleNameConfigUuid;
  }
  if (action === 'meetingStart' || action === 'meetingStop' || action === 'meetingFinalize') {
    return COMPANION_V1_GATT_BOUNDARY.meetingControlUuid;
  }
  if (action === 'speakerPrompt' || action === 'speakerStop') {
    return COMPANION_V1_GATT_BOUNDARY.speakerControlUuid;
  }
  return COMPANION_V1_GATT_BOUNDARY.wakeWordControlUuid;
}

function fixturePayloadHex(request: CompanionV1ControlRequest): string {
  const requestId = request.requestId ?? 1;
  const bytes = [1, actionCommand(request.action), requestId & 0xff, (requestId >> 8) & 0xff];
  if (request.action === 'stageBleName' && request.bleName) {
    bytes.push(...new TextEncoder().encode(request.bleName).slice(0, COMPANION_V1_BLE_NAME_MAX_BYTES));
  }
  return bytes.map(byte => byte.toString(16).padStart(2, '0')).join('');
}

function actionCommand(action: CompanionV1ControlAction): number {
  switch (action) {
    case 'stageBleName':
    case 'meetingStart':
    case 'speakerPrompt':
    case 'wakeArm':
      return 1;
    case 'applyBleName':
    case 'meetingStop':
    case 'speakerStop':
    case 'wakeDisable':
      return 2;
    case 'resetBleName':
    case 'meetingFinalize':
    case 'wakeTestDetect':
      return 3;
  }
}
