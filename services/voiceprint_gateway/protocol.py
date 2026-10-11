"""SAUC binary protocol boundary; client cannot choose a different customer's identity."""
import json
import struct

from .models import ServiceError

PCM_BYTES_PER_SECOND = 32000
SAMPLE_BYTES = 3 * PCM_BYTES_PER_SECOND
MAX_FRAME = 65536


def decode_client(frame: bytes, expected_type: int) -> tuple[int, bytes]:
    if not 8 <= len(frame) <= MAX_FRAME or frame[0] != 0x11:
        raise ServiceError(400, "invalid_audio_frame")
    message_type, flags = frame[1] >> 4, frame[1] & 15
    serialization, compression = frame[2] >> 4, frame[2] & 15
    if (message_type != expected_type or flags not in {0, 1, 2, 3} or compression != 0
            or frame[3] != 0 or serialization != (1 if expected_type == 1 else 0)):
        raise ServiceError(400, "unsupported_audio_frame")
    offset = 4
    if flags in {1, 3}:
        if len(frame) < 12:
            raise ServiceError(400, "invalid_audio_frame")
        sequence = struct.unpack(">i", frame[4:8])[0]
        if (flags == 1 and sequence <= 0) or (flags == 3 and sequence >= 0):
            raise ServiceError(400, "invalid_audio_sequence")
        offset += 4
    size = struct.unpack(">I", frame[offset:offset + 4])[0]
    if size != len(frame) - offset - 4:
        raise ServiceError(400, "invalid_audio_frame")
    if expected_type == 1 and flags not in {0, 1}:
        raise ServiceError(400, "invalid_start_frame")
    return flags, frame[offset + 4:]


def bind_first_frame(frame: bytes, profile: dict, session_id: str) -> bytes:
    _, payload = decode_client(frame, 1)
    try:
        incoming = json.loads(payload)
        options = incoming.get("request", {})
        if not isinstance(options, dict):
            raise ValueError()
    except (ValueError, AttributeError, UnicodeError):
        raise ServiceError(400, "invalid_start_json") from None
    # Explicit allowlist. Never forward caller voiceprints, uid, app, audio URLs, or secrets.
    allowed = {key: value for key, value in options.items()
               if key in {"enable_itn", "enable_punc", "enable_ddc", "end_window_size", "context"}}
    allowed.update(model_name="bigmodel", enable_nonstream=True, enable_speaker_info=True,
                   show_utterances=True, ssd_mode=2, voiceprints=[{"id": profile["id"]}],
                   result_type="full")
    body = json.dumps({"user": {"uid": session_id},
                       "audio": {"format": "pcm", "codec": "raw", "rate": 16000,
                                 "bits": 16, "channel": 1}, "request": allowed},
                      separators=(",", ":")).encode()
    header = frame[:8] if frame[1] & 1 else frame[:4]
    return header + struct.pack(">I", len(body)) + body


def is_terminal(frame: bytes | str) -> bool:
    # Server full response with negative sequence / last flag, or error response.
    return (isinstance(frame, bytes) and len(frame) >= 4
            and (frame[1] >> 4 == 15 or frame[1] & 2 != 0))
