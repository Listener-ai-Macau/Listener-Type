use serde::{Deserialize, Serialize};

pub const COMPANION_V1_SCHEMA: &str = "companion.host.v1";
pub const COMPANION_V1_DEFAULT_BLE_NAME: &str = "companion";
pub const COMPANION_V1_BLE_NAME_MAX_BYTES: usize = 29;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1GattBoundary {
    pub settings_service_uuid: &'static str,
    pub ble_name_config_uuid: &'static str,
    pub meeting_service_uuid: &'static str,
    pub meeting_control_uuid: &'static str,
    pub meeting_status_uuid: &'static str,
    pub sensors_service_uuid: &'static str,
    pub imu_status_uuid: &'static str,
    pub audio_output_service_uuid: &'static str,
    pub speaker_control_uuid: &'static str,
    pub speaker_status_uuid: &'static str,
    pub wake_word_control_uuid: &'static str,
    pub wake_word_status_uuid: &'static str,
}

pub const COMPANION_V1_GATT_BOUNDARY: CompanionV1GattBoundary = CompanionV1GattBoundary {
    settings_service_uuid: "8f7a0004-7b7d-4f3d-9d6f-6c2d1b7c0000",
    ble_name_config_uuid: "8f7a4002-7b7d-4f3d-9d6f-6c2d1b7c0000",
    meeting_service_uuid: "8f7a0008-7b7d-4f3d-9d6f-6c2d1b7c0000",
    meeting_control_uuid: "8f7a8001-7b7d-4f3d-9d6f-6c2d1b7c0000",
    meeting_status_uuid: "8f7a8002-7b7d-4f3d-9d6f-6c2d1b7c0000",
    sensors_service_uuid: "8f7a0009-7b7d-4f3d-9d6f-6c2d1b7c0000",
    imu_status_uuid: "8f7a9001-7b7d-4f3d-9d6f-6c2d1b7c0000",
    audio_output_service_uuid: "8f7a000a-7b7d-4f3d-9d6f-6c2d1b7c0000",
    speaker_control_uuid: "8f7aa001-7b7d-4f3d-9d6f-6c2d1b7c0000",
    speaker_status_uuid: "8f7aa002-7b7d-4f3d-9d6f-6c2d1b7c0000",
    wake_word_control_uuid: "8f7aa003-7b7d-4f3d-9d6f-6c2d1b7c0000",
    wake_word_status_uuid: "8f7aa004-7b7d-4f3d-9d6f-6c2d1b7c0000",
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1Snapshot {
    pub schema: &'static str,
    pub source: &'static str,
    pub connected: bool,
    pub write_supported: bool,
    pub detail: Option<String>,
    pub gatt: CompanionV1GattBoundary,
    pub meeting: CompanionV1MeetingState,
    pub imu: CompanionV1ImuState,
    pub speaker: CompanionV1SpeakerState,
    pub wake_word: CompanionV1WakeWordState,
    pub ble_name: CompanionV1BleNameState,
    pub last_control: Option<CompanionV1ControlEcho>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1MeetingState {
    pub state: &'static str,
    pub meeting_id: u32,
    pub segment_count: u16,
    pub duration_ms: u32,
    pub summary_state: &'static str,
    pub sync_state: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1ImuState {
    pub sensor_state: &'static str,
    pub motion_flags: Vec<&'static str>,
    pub sample_rate_hz: u16,
    pub calibration_state: &'static str,
    pub sample_timestamp_ms: u32,
    pub accel_mg: [i16; 3],
    pub gyro_dps: [i16; 3],
    pub temperature_c_x10: i16,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1SpeakerState {
    pub state: &'static str,
    pub prompt_id: u8,
    pub volume_percent: u8,
    pub played_count: u32,
    pub last_error: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1WakeWordState {
    pub armed: bool,
    pub engine_state: &'static str,
    pub sensitivity: u8,
    pub last_event: &'static str,
    pub last_confidence: u8,
    pub detection_count: u32,
    pub rejected_count: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1BleNameState {
    pub active_name: String,
    pub staged_name: String,
    pub max_len: u8,
    pub pending_restart: bool,
    pub status: &'static str,
    pub policy: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1ControlEcho {
    pub action: CompanionV1ControlAction,
    pub characteristic_uuid: &'static str,
    pub payload_hex: String,
    pub transport: &'static str,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CompanionV1ControlAction {
    StageBleName,
    ApplyBleName,
    ResetBleName,
    MeetingStart,
    MeetingStop,
    MeetingFinalize,
    SpeakerPrompt,
    SpeakerStop,
    WakeArm,
    WakeDisable,
    WakeTestDetect,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionV1ControlRequest {
    pub action: CompanionV1ControlAction,
    pub request_id: Option<u32>,
    pub ble_name: Option<String>,
    pub max_seconds: Option<u16>,
    pub profile_id: Option<u8>,
    pub volume_percent: Option<u8>,
    pub prompt_id: Option<u8>,
    pub sensitivity: Option<u8>,
}

pub fn companion_ble_name_is_valid(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > COMPANION_V1_BLE_NAME_MAX_BYTES {
        return false;
    }
    bytes.iter().all(|byte| {
        (0x20..=0x7e).contains(byte)
            && !matches!(*byte, b'"' | b'\'' | b';' | b'=' | b'\\')
    })
}

pub fn fixture_snapshot() -> CompanionV1Snapshot {
    CompanionV1Snapshot {
        schema: COMPANION_V1_SCHEMA,
        source: "fixture",
        connected: false,
        write_supported: true,
        detail: Some(
            "No Companion hardware is online; this fixture mirrors the firmware V1 contract."
                .to_string(),
        ),
        gatt: COMPANION_V1_GATT_BOUNDARY,
        meeting: CompanionV1MeetingState {
            state: "idle",
            meeting_id: 0,
            segment_count: 0,
            duration_ms: 0,
            summary_state: "notStarted",
            sync_state: "none",
        },
        imu: CompanionV1ImuState {
            sensor_state: "notPopulatedOnCurrentDevBoard",
            motion_flags: Vec::new(),
            sample_rate_hz: 0,
            calibration_state: "pendingHardware",
            sample_timestamp_ms: 0,
            accel_mg: [0, 0, 0],
            gyro_dps: [0, 0, 0],
            temperature_c_x10: 0,
        },
        speaker: CompanionV1SpeakerState {
            state: "promptReadyPendingHardware",
            prompt_id: 7,
            volume_percent: 50,
            played_count: 1,
            last_error: "none",
        },
        wake_word: CompanionV1WakeWordState {
            armed: true,
            engine_state: "armedFixture",
            sensitivity: 50,
            last_event: "none",
            last_confidence: 0,
            detection_count: 0,
            rejected_count: 0,
        },
        ble_name: CompanionV1BleNameState {
            active_name: COMPANION_V1_DEFAULT_BLE_NAME.to_string(),
            staged_name: COMPANION_V1_DEFAULT_BLE_NAME.to_string(),
            max_len: COMPANION_V1_BLE_NAME_MAX_BYTES as u8,
            pending_restart: false,
            status: "active",
            policy: "printableAscii1To29NoQuoteSemicolonEqualsBackslash",
        },
        last_control: None,
    }
}

pub fn apply_fixture_control(
    request: CompanionV1ControlRequest,
) -> Result<CompanionV1Snapshot, String> {
    let mut snapshot = fixture_snapshot();
    let request_id = request.request_id.unwrap_or(1);
    let payload = payload_for_control(&request, request_id)?;
    let characteristic_uuid = characteristic_for_action(request.action);

    match request.action {
        CompanionV1ControlAction::StageBleName => {
            let name = request
                .ble_name
                .as_deref()
                .ok_or_else(|| "Companion BLE name is required".to_string())?;
            if !companion_ble_name_is_valid(name) {
                return Err(
                    "Companion BLE name must be 1-29 printable ASCII bytes and cannot contain quote, apostrophe, semicolon, equals, or backslash.".to_string(),
                );
            }
            snapshot.ble_name.staged_name = name.to_string();
            snapshot.ble_name.status = "staged";
            snapshot.ble_name.pending_restart = true;
        }
        CompanionV1ControlAction::ApplyBleName => {
            snapshot.ble_name.status = "restartRequired";
            snapshot.ble_name.pending_restart = true;
        }
        CompanionV1ControlAction::ResetBleName => {
            snapshot.ble_name.staged_name = COMPANION_V1_DEFAULT_BLE_NAME.to_string();
            snapshot.ble_name.status = "staged";
            snapshot.ble_name.pending_restart = true;
        }
        CompanionV1ControlAction::MeetingStart => {
            snapshot.meeting.state = "recording";
            snapshot.meeting.meeting_id = 1;
            snapshot.meeting.segment_count = 0;
            snapshot.meeting.summary_state = "notStarted";
            snapshot.meeting.sync_state = "recording";
        }
        CompanionV1ControlAction::MeetingStop => {
            snapshot.meeting.state = "stored";
            snapshot.meeting.meeting_id = 1;
            snapshot.meeting.segment_count = 1;
            snapshot.meeting.duration_ms = 2400;
            snapshot.meeting.summary_state = "pendingHostSummary";
            snapshot.meeting.sync_state = "readyForHostSync";
        }
        CompanionV1ControlAction::MeetingFinalize => {
            snapshot.meeting.state = "finalized";
            snapshot.meeting.meeting_id = 1;
            snapshot.meeting.segment_count = 1;
            snapshot.meeting.duration_ms = 2400;
            snapshot.meeting.summary_state = "pendingHostSummary";
            snapshot.meeting.sync_state = "readyForHostSync";
        }
        CompanionV1ControlAction::SpeakerPrompt => {
            snapshot.speaker.state = "promptQueuedFixture";
            snapshot.speaker.prompt_id = request.prompt_id.unwrap_or(snapshot.speaker.prompt_id);
            snapshot.speaker.volume_percent =
                request.volume_percent.unwrap_or(snapshot.speaker.volume_percent).min(100);
            snapshot.speaker.played_count = snapshot.speaker.played_count.saturating_add(1);
        }
        CompanionV1ControlAction::SpeakerStop => {
            snapshot.speaker.state = "idle";
        }
        CompanionV1ControlAction::WakeArm => {
            snapshot.wake_word.armed = true;
            snapshot.wake_word.engine_state = "armedFixture";
            snapshot.wake_word.sensitivity = request.sensitivity.unwrap_or(50).min(100);
            snapshot.wake_word.last_event = "armed";
        }
        CompanionV1ControlAction::WakeDisable => {
            snapshot.wake_word.armed = false;
            snapshot.wake_word.engine_state = "disabledFixture";
            snapshot.wake_word.last_event = "disabled";
        }
        CompanionV1ControlAction::WakeTestDetect => {
            snapshot.wake_word.armed = true;
            snapshot.wake_word.engine_state = "detectedFixture";
            snapshot.wake_word.last_event = "detected";
            snapshot.wake_word.last_confidence = 88;
            snapshot.wake_word.detection_count = snapshot.wake_word.detection_count.saturating_add(1);
        }
    }

    snapshot.last_control = Some(CompanionV1ControlEcho {
        action: request.action,
        characteristic_uuid,
        payload_hex: payload_hex(&payload),
        transport: "fixtureNoHardware",
    });
    Ok(snapshot)
}

fn characteristic_for_action(action: CompanionV1ControlAction) -> &'static str {
    match action {
        CompanionV1ControlAction::StageBleName
        | CompanionV1ControlAction::ApplyBleName
        | CompanionV1ControlAction::ResetBleName => {
            COMPANION_V1_GATT_BOUNDARY.ble_name_config_uuid
        }
        CompanionV1ControlAction::MeetingStart
        | CompanionV1ControlAction::MeetingStop
        | CompanionV1ControlAction::MeetingFinalize => {
            COMPANION_V1_GATT_BOUNDARY.meeting_control_uuid
        }
        CompanionV1ControlAction::SpeakerPrompt | CompanionV1ControlAction::SpeakerStop => {
            COMPANION_V1_GATT_BOUNDARY.speaker_control_uuid
        }
        CompanionV1ControlAction::WakeArm
        | CompanionV1ControlAction::WakeDisable
        | CompanionV1ControlAction::WakeTestDetect => {
            COMPANION_V1_GATT_BOUNDARY.wake_word_control_uuid
        }
    }
}

fn payload_for_control(
    request: &CompanionV1ControlRequest,
    request_id: u32,
) -> Result<Vec<u8>, String> {
    match request.action {
        CompanionV1ControlAction::StageBleName
        | CompanionV1ControlAction::ApplyBleName
        | CompanionV1ControlAction::ResetBleName => {
            let command = match request.action {
                CompanionV1ControlAction::StageBleName => 1,
                CompanionV1ControlAction::ApplyBleName => 2,
                CompanionV1ControlAction::ResetBleName => 3,
                _ => unreachable!(),
            };
            let mut bytes = Vec::with_capacity(41);
            bytes.push(1);
            bytes.push(command);
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&request_id.to_le_bytes());
            let name = request.ble_name.as_deref().unwrap_or("");
            bytes.push(name.as_bytes().len() as u8);
            bytes.push(COMPANION_V1_BLE_NAME_MAX_BYTES as u8);
            bytes.push(0);
            bytes.push(0);
            let mut fixed = [0u8; COMPANION_V1_BLE_NAME_MAX_BYTES];
            let copy_len = name.as_bytes().len().min(COMPANION_V1_BLE_NAME_MAX_BYTES);
            fixed[..copy_len].copy_from_slice(&name.as_bytes()[..copy_len]);
            bytes.extend_from_slice(&fixed);
            Ok(bytes)
        }
        CompanionV1ControlAction::MeetingStart
        | CompanionV1ControlAction::MeetingStop
        | CompanionV1ControlAction::MeetingFinalize => {
            let command = match request.action {
                CompanionV1ControlAction::MeetingStart => 1,
                CompanionV1ControlAction::MeetingStop => 2,
                CompanionV1ControlAction::MeetingFinalize => 3,
                _ => unreachable!(),
            };
            let mut bytes = Vec::with_capacity(12);
            bytes.push(1);
            bytes.push(command);
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&request_id.to_le_bytes());
            bytes.extend_from_slice(&request.max_seconds.unwrap_or(7200).to_le_bytes());
            bytes.push(request.profile_id.unwrap_or(0));
            bytes.push(0);
            Ok(bytes)
        }
        CompanionV1ControlAction::SpeakerPrompt | CompanionV1ControlAction::SpeakerStop => {
            let command = match request.action {
                CompanionV1ControlAction::SpeakerPrompt => 1,
                CompanionV1ControlAction::SpeakerStop => 2,
                _ => unreachable!(),
            };
            let mut bytes = Vec::with_capacity(12);
            bytes.push(1);
            bytes.push(command);
            bytes.push(request.volume_percent.unwrap_or(50).min(100));
            bytes.push(request.prompt_id.unwrap_or(7));
            bytes.extend_from_slice(&request_id.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            Ok(bytes)
        }
        CompanionV1ControlAction::WakeArm
        | CompanionV1ControlAction::WakeDisable
        | CompanionV1ControlAction::WakeTestDetect => {
            let command = match request.action {
                CompanionV1ControlAction::WakeArm => 1,
                CompanionV1ControlAction::WakeDisable => 2,
                CompanionV1ControlAction::WakeTestDetect => 3,
                _ => unreachable!(),
            };
            let mut bytes = Vec::with_capacity(12);
            bytes.push(1);
            bytes.push(command);
            bytes.push(request.sensitivity.unwrap_or(50).min(100));
            bytes.push(0);
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&request_id.to_le_bytes());
            Ok(bytes)
        }
    }
}

fn payload_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ble_name_policy_matches_companion_contract() {
        assert!(companion_ble_name_is_valid("companion-v1"));
        assert!(companion_ble_name_is_valid("companion lab"));
        assert!(!companion_ble_name_is_valid(""));
        assert!(!companion_ble_name_is_valid("companion=lab"));
        assert!(!companion_ble_name_is_valid("companion-123456789012345678901"));
    }

    #[test]
    fn fixture_stage_ble_name_uses_ble_name_characteristic() {
        let snapshot = apply_fixture_control(CompanionV1ControlRequest {
            action: CompanionV1ControlAction::StageBleName,
            request_id: Some(9),
            ble_name: Some("companion-v1".to_string()),
            max_seconds: None,
            profile_id: None,
            volume_percent: None,
            prompt_id: None,
            sensitivity: None,
        })
        .expect("valid stage");
        assert_eq!(snapshot.ble_name.staged_name, "companion-v1");
        assert!(snapshot.ble_name.pending_restart);
        let echo = snapshot.last_control.expect("echo");
        assert_eq!(echo.characteristic_uuid, COMPANION_V1_GATT_BOUNDARY.ble_name_config_uuid);
        assert!(echo.payload_hex.starts_with("01010000090000000c1d"));
    }

    #[test]
    fn fixture_meeting_is_separate_from_dictation_contract() {
        let snapshot = apply_fixture_control(CompanionV1ControlRequest {
            action: CompanionV1ControlAction::MeetingFinalize,
            request_id: Some(11),
            ble_name: None,
            max_seconds: Some(3600),
            profile_id: Some(2),
            volume_percent: None,
            prompt_id: None,
            sensitivity: None,
        })
        .expect("finalize");
        assert_eq!(snapshot.meeting.state, "finalized");
        assert_eq!(snapshot.meeting.sync_state, "readyForHostSync");
        assert_eq!(snapshot.last_control.unwrap().characteristic_uuid, COMPANION_V1_GATT_BOUNDARY.meeting_control_uuid);
    }
}
