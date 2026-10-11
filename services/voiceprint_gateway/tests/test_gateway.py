import asyncio
import hashlib
import io
import json
import struct
import tempfile
import time
import unittest
import wave
from contextlib import asynccontextmanager
from pathlib import Path
from uuid import uuid4

import jwt
from cryptography.hazmat.primitives.asymmetric import rsa
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect

from services.voiceprint_gateway.app import create_app, production_app
from services.voiceprint_gateway.auth import JwtAuthenticator
from services.voiceprint_gateway.models import Principal, ProviderUnknown, ServiceError
from services.voiceprint_gateway.protocol import SAMPLE_BYTES, bind_first_frame
from services.voiceprint_gateway.store import Store

FP = hashlib.sha256(b"operator-app").hexdigest()
BYOK_FP = hashlib.sha256(b"customer-app").hexdigest()
PCM = b"\x12\x00" * (SAMPLE_BYTES // 2)


def frame(message_type, payload, last=False):
    return bytes([0x11, (message_type << 4) | (2 if last else 0), 0x10 if message_type == 1 else 0, 0]) + struct.pack(">I", len(payload)) + payload


class FakeAuth:
    async def authenticate(self, authorization):
        # Only injected in these tests; production factory always constructs verified JWT auth.
        users = {"Bearer a": Principal("customer-a", "period-1", 360, True),
                 "Bearer b": Principal("customer-b", "period-1", 360, True),
                 "Bearer free": Principal("free-customer", "", 0, False)}
        if authorization not in users:
            raise ServiceError(401, "login_required")
        return users[authorization]


class MemoryStorage:
    def __init__(self):
        self.objects = {}
        self.saved = []

    async def put(self, key, audio):
        self.objects[key] = audio
        self.saved.append((key, audio))

    async def get(self, key):
        return self.objects[key]

    async def url(self, key):
        return "https://private.example.test/" + key + "?temporary-signature=test"

    async def delete(self, key):
        self.objects.pop(key, None)


class FakeStream:
    def __init__(self, provider):
        self.provider, self.queue = provider, asyncio.Queue()

    async def send(self, packet):
        self.provider.frames.append(packet)
        if packet[1] >> 4 == 2 and packet[1] & 2:
            await self.queue.put(bytes([0x11, 0x93, 0x10, 0]) + b"test-final")

    def __aiter__(self):
        return self

    async def __anext__(self):
        return await self.queue.get()


class FakeProvider:
    fingerprint = FP

    def __init__(self):
        self.created, self.deleted, self.frames = [], [], []
        self.unknown, self.connections = False, 0

    async def register(self, audio_url, name):
        self.created.append((audio_url, name))
        if self.unknown:
            raise ProviderUnknown()
        return {"id": str(uuid4()), "speakerName": name, "accountFingerprint": FP}

    async def delete(self, profile):
        self.deleted.append(profile)

    @asynccontextmanager
    async def stream(self, session_id):
        self.connections += 1
        yield FakeStream(self)


class GatewayTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.store = Store(str(Path(self.directory.name) / "test.sqlite3"))
        self.auth, self.storage, self.provider = FakeAuth(), MemoryStorage(), FakeProvider()
        self.app = create_app(auth=self.auth, storage=self.storage, provider=self.provider,
                              store=self.store, public_url="https://cloud.example.test",
                              ticket_secret=b"test-secret-is-at-least-thirty-two-bytes")
        self.client = TestClient(self.app)
        self.client.__enter__()

    def tearDown(self):
        self.client.__exit__(None, None, None)
        self.store.db.close()
        self.directory.cleanup()

    @staticmethod
    def headers(user="a"):
        return {"Authorization": "Bearer " + user}

    def begin(self, user="a", mode="managed", request_id=None):
        body = {"requestId": request_id or str(uuid4()), "mode": mode,
                "consentVersion": "voiceprint-cloud-v1"}
        if mode == "byok":
            body["accountFingerprint"] = BYOK_FP
        response = self.client.post("/v1/voiceprints/enrollments", json=body, headers=self.headers(user))
        self.assertEqual(response.status_code, 200, response.text)
        return response.json(), body

    def upload(self, enrollment, step, user="a", pcm=PCM):
        return self.client.put(f"/v1/voiceprints/enrollments/{enrollment['id']}/samples/{step}", content=pcm,
                               headers={**self.headers(user), "Content-Type": "audio/pcm"})

    def enroll(self, user="a", mode="managed"):
        data, _ = self.begin(user, mode)
        for step in range(3):
            response = self.upload(data, step, user)
            self.assertEqual(response.status_code, 200, response.text)
        return response.json()

    def session(self, user="a", request_id=None):
        return self.client.post("/v1/asr/sessions", json={"requestId": request_id or str(uuid4())},
                                headers=self.headers(user))

    def start_byok(self, data, user="a"):
        response = self.client.post(f"/v1/voiceprints/enrollments/{data['id']}/byok-registration-start",
                                    headers=self.headers(user))
        self.assertEqual(response.status_code, 200, response.text)
        return response.json()

    def test_three_guided_samples_join_and_register_exactly_once(self):
        request_id = str(uuid4())
        data, _ = self.begin(request_id=request_id)
        duplicate, _ = self.begin(request_id=request_id)
        self.assertEqual(data, duplicate)
        self.assertEqual(self.upload(data, 0).json()["state"], "collecting")
        self.upload(data, 1)
        ready = self.upload(data, 2).json()
        self.assertEqual(ready["state"], "ready")
        self.assertEqual(self.upload(data, 2).json(), ready)
        self.assertEqual(len(self.provider.created), 1)
        joined = next(value for key, value in self.storage.saved if key.endswith("joined.wav"))
        with wave.open(io.BytesIO(joined), "rb") as wav:
            self.assertEqual((wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getnframes()),
                             (1, 2, 16000, 144000))
            self.assertEqual(wav.readframes(wav.getnframes()), PCM * 3)
        self.assertFalse(self.storage.objects, "raw enrollment uploads must be deleted after ready")
        self.assertNotIn("audioUrl", ready)
        self.assertNotIn("Embedding", json.dumps(ready))

    def test_two_customers_have_independent_profiles_and_scoped_operations(self):
        a = self.enroll("a")
        b = self.enroll("b")
        self.assertNotEqual(a["profile"]["id"], b["profile"]["id"])
        self.assertNotEqual(a["profile"]["speakerName"], b["profile"]["speakerName"])
        for verb in (self.client.get, self.client.delete):
            response = verb("/v1/voiceprints/enrollments/" + a["id"], headers=self.headers("b"))
            self.assertEqual(response.status_code, 404)
        self.assertEqual(self.upload(a, 0, "b").status_code, 404)
        sa, sb = self.session("a").json(), self.session("b").json()
        self.assertEqual(sa["profile"], a["profile"])
        self.assertEqual(sb["profile"], b["profile"])
        self.assertEqual(self.client.delete("/v1/voiceprints/binding", headers=self.headers("a")).status_code, 200)
        self.assertEqual(self.client.get("/v1/voiceprints/binding", headers=self.headers("b")).json()["profile"], b["profile"])

    def test_byok_never_calls_managed_provider_or_spends_managed_quota(self):
        data = self.enroll("free", "byok")
        self.assertEqual(data["state"], "awaitingProvider")
        self.assertNotIn("audioUrl", data)
        data = self.start_byok(data, "free")
        self.assertTrue(data["audioUrl"].startswith("https://"))
        confirmation = {"id": str(uuid4()), "speakerName": data["speakerName"], "accountFingerprint": BYOK_FP}
        url = f"/v1/voiceprints/enrollments/{data['id']}/byok-confirmation"
        self.assertEqual(self.client.post(url, json=confirmation, headers=self.headers("a")).status_code, 404)
        ready = self.client.post(url, json=confirmation, headers=self.headers("free"))
        self.assertEqual(ready.json()["state"], "ready")
        self.assertEqual(self.client.post(url, json=confirmation, headers=self.headers("free")).json(), ready.json())
        self.assertFalse(self.provider.created)
        self.assertFalse(self.storage.objects)
        self.assertEqual(self.session("free").status_code, 403)
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM usage").fetchone()[0], 0)

    def test_byok_claimed_identity_cannot_become_managed_authority(self):
        data = self.enroll(mode="byok")
        data = self.start_byok(data)
        confirmation = {"id": str(uuid4()), "speakerName": data["speakerName"], "accountFingerprint": BYOK_FP}
        response = self.client.post(f"/v1/voiceprints/enrollments/{data['id']}/byok-confirmation",
                                    json=confirmation, headers=self.headers())
        self.assertEqual(response.status_code, 200)
        self.assertEqual(self.session().status_code, 409)
        response = self.client.post("/v1/asr/sessions", headers=self.headers(),
                                    json={"requestId": str(uuid4()), "voiceprintId": confirmation["id"]})
        self.assertEqual(response.status_code, 422)

    def test_malformed_or_conflicting_sample_and_fourth_step_are_rejected(self):
        data, _ = self.begin()
        self.assertEqual(self.upload(data, 0, pcm=b"short").status_code, 400)
        self.assertEqual(self.upload(data, 0, pcm=PCM + b"00").status_code, 413)
        self.assertEqual(self.upload(data, 3).status_code, 400)
        self.upload(data, 0)
        self.assertEqual(self.upload(data, 0, pcm=b"\0" * SAMPLE_BYTES).status_code, 409)
        self.assertEqual(len(self.storage.objects), 1)

    def test_unknown_registration_is_not_retried_and_old_binding_survives(self):
        old = self.enroll()["profile"]
        self.provider.unknown = True
        data = self.enroll()
        self.assertEqual(data["state"], "unknown")
        self.assertEqual(self.upload(data, 2).json()["state"], "unknown")
        self.assertEqual(len(self.provider.created), 2)
        self.assertEqual(self.client.get("/v1/voiceprints/binding", headers=self.headers()).json()["profile"], old)
        response = self.client.post("/v1/voiceprints/enrollments", json={"requestId": str(uuid4()),
                                    "mode": "managed", "consentVersion": "voiceprint-cloud-v1"}, headers=self.headers())
        self.assertEqual(response.status_code, 409)

    def test_subscription_and_idempotent_quota_reservations(self):
        self.enroll()
        request_id = str(uuid4())
        first = self.session(request_id=request_id).json()
        duplicate = self.session(request_id=request_id).json()
        self.assertEqual(first, duplicate)
        self.assertEqual(self.session().status_code, 200)
        self.assertEqual(self.session().status_code, 402)
        self.assertEqual(self.session("free").status_code, 403)
        self.assertEqual(self.client.post("/v1/voiceprints/enrollments", headers=self.headers("free"),
                         json={"requestId": str(uuid4()), "mode": "managed", "consentVersion": "voiceprint-cloud-v1"}).status_code, 403)

    def test_expired_unused_ticket_releases_quota(self):
        profile = self.enroll()["profile"]
        p = Principal("customer-a", "period-1", 180, True)
        first = self.store.reserve(p, str(uuid4()), profile, 0)
        second = self.store.reserve(p, str(uuid4()), profile, 61)
        self.assertNotEqual(first["id"], second["id"])
        self.assertEqual(self.store.session(p.customer, first["id"])["state"], "finished")

    def test_stream_binds_owned_id_forwards_final_and_charges_once(self):
        profile = self.enroll()["profile"]
        session = self.session().json()
        path = f"/v1/asr/sessions/{session['id']}/stream"
        headers = {**self.headers(), "X-Listener-Session-Token": session["sessionToken"]}
        with self.client.websocket_connect(path, headers=headers) as socket:
            malicious = {"user": {"uid": "another-customer"}, "audio": {"url": "https://attacker.test"},
                         "request": {"voiceprints": [{"id": "another-voice"}], "ssd_mode": 0,
                                     "enable_speaker_info": False, "enable_nonstream": False}}
            socket.send_bytes(frame(1, json.dumps(malicious).encode()))
            socket.send_bytes(frame(2, PCM[:32000], last=True))
            self.assertTrue(socket.receive_bytes().startswith(b"\x11\x93"))
        sent = json.loads(self.provider.frames[0][8:])
        self.assertEqual(sent["request"]["voiceprints"], [{"id": profile["id"]}])
        self.assertEqual(sent["user"]["uid"], session["id"])
        self.assertTrue(sent["request"]["enable_nonstream"])
        self.assertEqual(sent["request"]["ssd_mode"], 2)
        self.assertNotIn("url", sent["audio"])
        used, reserved = self.store.db.execute("SELECT used,reserved FROM usage").fetchone()
        self.assertEqual((used, reserved), (1, 0))
        self.store.finish("customer-a", session["id"])
        self.assertEqual(self.store.db.execute("SELECT used,reserved FROM usage").fetchone(), (1, 0))
        with self.assertRaises(WebSocketDisconnect):
            with self.client.websocket_connect(path, headers=headers):
                pass
        self.assertEqual(self.provider.connections, 1)

    def test_wrong_customer_ticket_or_deleted_binding_cannot_open_provider(self):
        self.enroll()
        self.enroll("b")
        s = self.session().json()
        path = f"/v1/asr/sessions/{s['id']}/stream"
        for headers in ({**self.headers("b"), "X-Listener-Session-Token": s["sessionToken"]},
                        {**self.headers(), "X-Listener-Session-Token": "wrong"}):
            with self.assertRaises(WebSocketDisconnect):
                with self.client.websocket_connect(path, headers=headers):
                    pass
        self.client.delete("/v1/voiceprints/binding", headers=self.headers())
        with self.assertRaises(WebSocketDisconnect):
            with self.client.websocket_connect(path, headers={**self.headers(), "X-Listener-Session-Token": s["sessionToken"]}):
                pass
        self.assertEqual(self.provider.connections, 0)

    def test_retried_native_audio_sequence_is_forwarded_and_charged_once(self):
        self.enroll()
        s = self.session().json()
        headers = {**self.headers(), "X-Listener-Session-Token": s["sessionToken"]}
        with self.client.websocket_connect(f"/v1/asr/sessions/{s['id']}/stream", headers=headers) as socket:
            first = b"\x11\x11\x10\x00" + struct.pack(">iI", 1, 2) + b"{}"
            audio = b"\x11\x21\x00\x00" + struct.pack(">iI", 2, 32000) + PCM[:32000]
            end = b"\x11\x23\x00\x00" + struct.pack(">iI", -3, 0)
            socket.send_bytes(first)
            socket.send_bytes(audio)
            socket.send_bytes(audio)
            socket.send_bytes(end)
            socket.receive_bytes()
        self.assertEqual(len(self.provider.frames), 3)
        self.assertEqual(self.provider.frames[1], audio)
        self.assertEqual(self.store.db.execute("SELECT used,reserved FROM usage").fetchone(), (1, 0))

    def test_protocol_rejects_bad_headers_lengths_and_compression(self):
        profile = {"id": str(uuid4())}
        good = frame(1, b"{}")
        for bad in (good[:-1], bytes([0x12]) + good[1:], good[:2] + b"\x11" + good[3:], frame(2, b"00")):
            with self.assertRaises(ServiceError):
                bind_first_frame(bad, profile, "owned-session")

    def test_native_positive_and_negative_sequence_protocol_is_preserved(self):
        from services.voiceprint_gateway.protocol import decode_client
        profile = {"id": str(uuid4())}
        first = b"\x11\x11\x10\x00" + struct.pack(">iI", 1, 2) + b"{}"
        bound = bind_first_frame(first, profile, "owned-session")
        self.assertEqual(bound[:8], first[:8])
        flags, body = decode_client(bound, 1)
        self.assertEqual(flags, 1)
        self.assertEqual(json.loads(body)["request"]["voiceprints"], [{"id": profile["id"]}])
        end = b"\x11\x23\x00\x00" + struct.pack(">iI", -2, 4) + b"1234"
        self.assertEqual(decode_client(end, 2), (3, b"1234"))

    def test_production_configuration_has_no_fake_auth_fallback(self):
        from unittest.mock import patch
        with patch.dict("os.environ", {}, clear=True):
            with self.assertRaises(KeyError):
                production_app()

    def test_concurrent_repeated_requests_register_once_and_reserve_atomically(self):
        from concurrent.futures import ThreadPoolExecutor
        data, _ = self.begin()
        self.upload(data, 0)
        self.upload(data, 1)
        with ThreadPoolExecutor(max_workers=4) as pool:
            results = list(pool.map(lambda _: self.upload(data, 2).json(), range(4)))
        self.assertTrue(all(row["state"] == "ready" for row in results))
        self.assertEqual(len(self.provider.created), 1)
        with ThreadPoolExecutor(max_workers=4) as pool:
            statuses = list(pool.map(lambda _: self.session().status_code, range(4)))
        self.assertEqual(sorted(statuses), [200, 200, 402, 402])

    def test_uncertain_byok_registration_is_locked_against_repeating_create(self):
        data = self.enroll(mode="byok")
        data = self.start_byok(data)
        start_url = f"/v1/voiceprints/enrollments/{data['id']}/byok-registration-start"
        self.assertEqual(self.client.post(start_url, headers=self.headers()).status_code, 409)
        url = f"/v1/voiceprints/enrollments/{data['id']}/byok-unknown"
        unknown = self.client.post(url, headers=self.headers()).json()
        self.assertEqual(unknown["state"], "unknown")
        self.assertNotIn("audioUrl", unknown)
        self.assertEqual(self.upload(data, 2).json()["state"], "unknown")


class AuthTests(unittest.TestCase):
    def test_verified_email_signed_entitlements_and_bounded_token_lifetime(self):
        from unittest.mock import Mock
        private_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        verifier = JwtAuthenticator("https://accounts.example.test", "listener-cloud", "https://accounts.example.test/jwks")
        verifier.keys = Mock()
        verifier.keys.get_signing_key_from_jwt.return_value.key = private_key.public_key()
        now = int(time.time())
        claims = {"iss": "https://accounts.example.test", "aud": "listener-cloud", "sub": "stable-customer",
                  "iat": now, "exp": now + 300, "email_verified": True,
                  "listener": {"billingPeriod": "2026-10", "includedAsrSeconds": 3600, "asrEnabled": True}}
        def token(value):
            return "Bearer " + jwt.encode(value, private_key, algorithm="RS256")
        p = verifier.verify(token(claims))
        self.assertEqual((p.included_seconds, p.managed_enabled), (3600, True))
        self.assertNotEqual(p.customer, claims["sub"])
        for change in ({"email_verified": False}, {"aud": "other"}, {"exp": now + 3600},
                       {"listener": {"includedAsrSeconds": "3600"}}, {"sub": ""}):
            with self.assertRaises(ServiceError):
                verifier.verify(token({**claims, **change}))
        wrong = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        with self.assertRaises(ServiceError):
            verifier.verify("Bearer " + jwt.encode(claims, wrong, algorithm="RS256"))


if __name__ == "__main__":
    unittest.main()
