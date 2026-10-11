import asyncio
import hashlib
import hmac
import io
import math
import os
import struct
import time
import wave
from uuid import UUID
from weakref import WeakValueDictionary

from fastapi import FastAPI, Header, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import JSONResponse

from .adapters import TosStorage, VolcProvider
from .auth import JwtAuthenticator, secure_url
from .models import (ByokConfirmation, EnrollmentInput, Principal, ProviderUnknown,
                     ServiceError, SessionInput)
from .protocol import (MAX_FRAME, PCM_BYTES_PER_SECOND, SAMPLE_BYTES, bind_first_frame,
                       decode_client, is_terminal)
from .store import Store


def create_app(*, auth, storage, provider, store: Store, public_url: str, ticket_secret: bytes) -> FastAPI:
    public_url = secure_url(public_url)
    if len(ticket_secret) < 32:
        raise ValueError("A random ticket secret of at least 32 bytes is required")
    app = FastAPI(title="Listener Voiceprint Gateway", version="0.1.0")
    locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()

    def lock(key: str) -> asyncio.Lock:
        current = locks.get(key)
        if current is None:
            current = asyncio.Lock()
            locks[key] = current
        return current

    async def principal(authorization: str | None) -> Principal:
        return await auth.authenticate(authorization)

    def ticket(session: dict) -> str:
        message = (session["id"] + "\0" + session["customer"]).encode()
        return hmac.new(ticket_secret, message, hashlib.sha256).hexdigest()

    async def view(data: dict, provide_byok_audio=False) -> dict:
        result = {"id": data["id"], "mode": data["mode"], "state": data["state"],
                  "receivedSteps": sorted(int(step) for step in data["samples"]),
                  "profile": data["profile"]}
        if data["mode"] == "byok" and provide_byok_audio:
            result.update(audioUrl=await storage.url(data["audioKey"]), speakerName=data["speakerName"],
                          accountFingerprint=data["fingerprint"], audioUrlExpiresIn=300)
        return result

    async def cleanup(data: dict):
        # An operator-configured private-bucket lifecycle is the crash/failure backstop.
        keys = [sample["key"] for sample in data["samples"].values()]
        if data["audioKey"]:
            keys.append(data["audioKey"])
        for key in keys:
            try:
                await storage.delete(key)
            except Exception:
                pass

    @app.exception_handler(ServiceError)
    async def known_error(_request: Request, error: ServiceError):
        return JSONResponse({"error": error.code}, status_code=error.status)

    @app.exception_handler(Exception)
    async def internal_error(_request: Request, _error: Exception):
        return JSONResponse({"error": "service_unavailable"}, status_code=503)

    @app.middleware("http")
    async def private_responses(request: Request, call_next):
        response = await call_next(request)
        response.headers["Cache-Control"] = "no-store"
        return response

    @app.get("/health")
    async def health():
        return {"status": "ok"}

    @app.get("/v1/account")
    async def account(authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        return {"customer": p.customer, "managedEnabled": p.managed_enabled,
                "billingPeriod": p.period, "includedAsrSeconds": p.included_seconds}

    @app.post("/v1/voiceprints/enrollments")
    async def begin(body: EnrollmentInput, authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        if body.mode == "managed":
            if not p.managed_enabled:
                raise ServiceError(403, "subscription_required")
            if body.accountFingerprint is not None:
                raise ServiceError(400, "managed_account_is_server_owned")
            fp = provider.fingerprint
        else:
            if body.accountFingerprint is None:
                raise ServiceError(400, "byok_account_required")
            fp = body.accountFingerprint
        data = store.create_enrollment(p.customer, str(body.requestId), body.mode, fp)
        return await view(data)

    @app.get("/v1/voiceprints/enrollments/{enrollment_id}")
    async def status(enrollment_id: UUID, authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        return await view(store.enrollment(p.customer, str(enrollment_id)))

    @app.put("/v1/voiceprints/enrollments/{enrollment_id}/samples/{step}")
    async def upload(enrollment_id: UUID, step: int, request: Request,
                     authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        sid = str(enrollment_id)
        store.enrollment(p.customer, sid)  # Reject wrong customer before accepting the body.
        if step not in range(3) or request.headers.get("content-type") != "audio/pcm":
            raise ServiceError(400, "three_pcm_samples_required")
        audio = bytearray()
        async for chunk in request.stream():
            if len(audio) + len(chunk) > SAMPLE_BYTES:
                raise ServiceError(413, "sample_too_large")
            audio.extend(chunk)
        if len(audio) != SAMPLE_BYTES:
            raise ServiceError(400, "sample_requires_three_seconds_16khz_mono_pcm16")
        digest = hashlib.sha256(audio).hexdigest()
        async with lock(sid):
            data = store.enrollment(p.customer, sid)
            existing = data["samples"].get(str(step))
            if existing:
                if existing["digest"] != digest:
                    raise ServiceError(409, "sample_conflict")
                return await view(data)
            if data["state"] != "collecting":
                raise ServiceError(409, "enrollment_not_collecting")
            if time.time() - data["created"] > 1200:
                data["state"] = "expired"
                store.save_enrollment(sid, data)
                await cleanup(data)
                raise ServiceError(410, "enrollment_expired")
            if data["mode"] == "managed" and not p.managed_enabled:
                raise ServiceError(403, "subscription_required")
            key = f"enrollment/{sid}/step-{step}-{digest}.pcm"
            await storage.put(key, bytes(audio))
            data["samples"][str(step)] = {"digest": digest, "key": key}
            store.save_enrollment(sid, data)
            if len(data["samples"]) < 3:
                return await view(data)
            try:
                pcm = b"".join([await storage.get(data["samples"][str(i)]["key"]) for i in range(3)])
                if len(pcm) != 3 * SAMPLE_BYTES:
                    raise ServiceError(503, "stored_sample_invalid")
                output = io.BytesIO()
                with wave.open(output, "wb") as wav:
                    wav.setnchannels(1)
                    wav.setsampwidth(2)
                    wav.setframerate(16000)
                    wav.writeframes(pcm)
                data["audioKey"] = f"enrollment/{sid}/joined.wav"
                await storage.put(data["audioKey"], output.getvalue())
                if data["mode"] == "byok":
                    data["state"] = "awaitingProvider"
                    store.save_enrollment(sid, data)
                    return await view(data)
                # Persist before non-idempotent provider create; restart leaves Unknown.
                data["state"] = "registering"
                store.save_enrollment(sid, data)
                profile = await provider.register(await storage.url(data["audioKey"]), data["speakerName"])
                store.publish(p.customer, data, profile)
            except BaseException as error:
                data["state"] = "unknown" if (data["state"] == "registering"
                                             and not isinstance(error, ServiceError)) else "failed"
                store.save_enrollment(sid, data)
                if data["state"] == "failed":
                    await cleanup(data)
                if isinstance(error, ProviderUnknown):
                    return await view(data)
                raise
            await cleanup(data)
            return await view(data)

    @app.post("/v1/voiceprints/enrollments/{enrollment_id}/byok-confirmation")
    async def confirm(enrollment_id: UUID, body: ByokConfirmation,
                      authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        sid = str(enrollment_id)
        async with lock(sid):
            data = store.enrollment(p.customer, sid)
            if (data["mode"] != "byok" or body.accountFingerprint != data["fingerprint"]
                    or body.speakerName != data["speakerName"]):
                raise ServiceError(409, "byok_identity_mismatch")
            profile = body.model_dump(mode="json")
            if data["state"] == "ready" and data["profile"] == profile:
                return await view(data)
            if data["state"] not in {"registering", "unknown"}:
                raise ServiceError(409, "byok_not_awaiting_confirmation")
            # Client-verified BYOK IDs can never become managed IDs or managed sessions.
            store.publish(p.customer, data, profile)
            await cleanup(data)
            return await view(data)

    @app.post("/v1/voiceprints/enrollments/{enrollment_id}/byok-registration-start")
    async def byok_start(enrollment_id: UUID, authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        sid = str(enrollment_id)
        async with lock(sid):
            data = store.enrollment(p.customer, sid)
            if data["mode"] != "byok" or data["state"] != "awaitingProvider":
                raise ServiceError(409, "byok_registration_already_started")
            # One grant before the client's non-idempotent provider create. A lost
            # response is uncertain, never a reason to grant another automatic create.
            data["state"] = "registering"
            store.save_enrollment(sid, data)
            return await view(data, provide_byok_audio=True)

    @app.delete("/v1/voiceprints/enrollments/{enrollment_id}")
    async def cancel(enrollment_id: UUID, authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        sid = str(enrollment_id)
        async with lock(sid):
            data = store.enrollment(p.customer, sid)
            if data["state"] in {"registering", "unknown", "ready"}:
                raise ServiceError(409, "enrollment_requires_reconciliation_or_binding_delete")
            data["state"] = "cancelled"
            store.save_enrollment(sid, data)
            await cleanup(data)
            return {"state": "cancelled"}

    @app.post("/v1/voiceprints/enrollments/{enrollment_id}/byok-unknown")
    async def byok_unknown(enrollment_id: UUID, authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        sid = str(enrollment_id)
        async with lock(sid):
            data = store.enrollment(p.customer, sid)
            if data["mode"] != "byok" or data["state"] not in {"registering", "unknown"}:
                raise ServiceError(409, "byok_not_awaiting_confirmation")
            data["state"] = "unknown"
            store.save_enrollment(sid, data)
            return await view(data)

    @app.get("/v1/voiceprints/binding")
    async def binding(mode: str = "managed", accountFingerprint: str | None = None,
                      authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        if mode not in {"managed", "byok"} or (mode == "byok" and not accountFingerprint):
            raise ServiceError(400, "invalid_route")
        fp = provider.fingerprint if mode == "managed" else accountFingerprint
        return {"mode": mode, "profile": store.binding(p.customer, mode, fp)}

    @app.delete("/v1/voiceprints/binding")
    async def delete_binding(mode: str = "managed", accountFingerprint: str | None = None,
                             authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        if mode not in {"managed", "byok"} or (mode == "byok" and not accountFingerprint):
            raise ServiceError(400, "invalid_route")
        fp = provider.fingerprint if mode == "managed" else accountFingerprint
        async with lock(p.customer + mode + fp):
            profile = store.binding(p.customer, mode, fp)
            # Remove service authority before provider delete, even on uncertain outcome.
            store.remove_binding(p.customer, mode, fp)
            if mode == "managed":
                try:
                    await provider.delete(profile)
                except ProviderUnknown:
                    return {"state": "unbound", "providerDeletion": "unknown"}
            return {"state": "unbound", "providerDeletion": "done" if mode == "managed" else "clientRequired"}

    @app.post("/v1/asr/sessions")
    async def session(body: SessionInput, authorization: str | None = Header(default=None)):
        p = await principal(authorization)
        if not p.managed_enabled:
            raise ServiceError(403, "subscription_required")
        profile = store.binding(p.customer, "managed", provider.fingerprint)
        s = store.reserve(p, str(body.requestId), profile, time.time())
        url = public_url.replace("https://", "wss://", 1)
        return {"id": s["id"], "websocketUrl": f"{url}/v1/asr/sessions/{s['id']}/stream",
                "sessionToken": ticket(s), "expiresAt": s["expires"],
                "maxAudioSeconds": s["reserved"], "profile": s["profile"]}

    @app.websocket("/v1/asr/sessions/{session_id}/stream")
    async def stream(socket: WebSocket, session_id: UUID):
        sid = str(session_id)
        try:
            p = await principal(socket.headers.get("authorization"))
            s = store.session(p.customer, sid)
            supplied = socket.headers.get("x-listener-session-token", "")
            if not hmac.compare_digest(supplied, ticket(s)):
                raise ServiceError(403, "invalid_session_ticket")
            # Enrollment/account changes invalidate outstanding authority.
            if store.binding(p.customer, "managed", provider.fingerprint) != s["profile"]:
                raise ServiceError(409, "voiceprint_changed")
            store.consume(p, sid, time.time())
        except ServiceError:
            await socket.close(code=4403)
            return
        await socket.accept()
        tasks = []
        try:
            async with asyncio.timeout(s["reserved"] + 15):
                first = await asyncio.wait_for(socket.receive_bytes(), timeout=5)
                start = bind_first_frame(first, s["profile"], sid)
                async with provider.stream(sid) as upstream:
                    await upstream.send(start)

                    async def audio_upload():
                        audio_bytes = 0
                        last_sequence = struct.unpack(">i", first[4:8])[0] if first[1] & 1 else 0
                        last_digest = None
                        while True:
                            frame = await socket.receive_bytes()
                            flags, pcm = decode_client(frame, 2)
                            if flags & 1:
                                sequence = abs(struct.unpack(">i", frame[4:8])[0])
                                digest = hashlib.sha256(frame).digest()
                                if sequence == last_sequence and digest == last_digest:
                                    # The native writer can retry the same sequence after a
                                    # send timeout. Forward/charge that PCM exactly once.
                                    continue
                                if sequence <= last_sequence:
                                    raise ServiceError(409, "audio_sequence_conflict")
                                last_sequence, last_digest = sequence, digest
                            if len(pcm) % 2:
                                raise ServiceError(400, "unaligned_pcm")
                            audio_bytes += len(pcm)
                            if audio_bytes > s["reserved"] * PCM_BYTES_PER_SECOND:
                                raise ServiceError(402, "session_audio_limit")
                            store.record_audio(sid, math.ceil(audio_bytes / PCM_BYTES_PER_SECOND))
                            await upstream.send(frame)
                            if flags & 2:
                                return

                    async def provider_download():
                        async for frame in upstream:
                            if not isinstance(frame, bytes) or len(frame) > 1024 * 1024:
                                raise ServiceError(502, "invalid_provider_frame")
                            await socket.send_bytes(frame)
                            if is_terminal(frame):
                                return

                    upload_task = asyncio.create_task(audio_upload())
                    download_task = asyncio.create_task(provider_download())
                    tasks = [upload_task, download_task]
                    done, _ = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
                    for task in done:
                        task.result()
                    if upload_task in done and download_task not in done:
                        await download_task
        except (ServiceError, WebSocketDisconnect, TimeoutError):
            pass
        except Exception:
            pass
        finally:
            for task in tasks:
                task.cancel()
            if tasks:
                await asyncio.gather(*tasks, return_exceptions=True)
            store.finish(p.customer, sid)
            try:
                await socket.close(code=1000)
            except RuntimeError:
                pass

    return app


def production_app() -> FastAPI:
    """No development-auth fallback. Missing operator/account configuration fails startup."""
    return create_app(
        auth=JwtAuthenticator(os.environ["AUTH_ISSUER"], os.environ["AUTH_AUDIENCE"], os.environ["AUTH_JWKS_URL"]),
        storage=TosStorage(),
        provider=VolcProvider(os.environ["VOLC_APP_ID"], os.environ["VOLC_ACCESS_KEY"], os.environ["VOLC_RESOURCE_ID"]),
        store=Store(os.environ["DATABASE_PATH"]), public_url=os.environ["PUBLIC_URL"],
        ticket_secret=bytes.fromhex(os.environ["SESSION_TICKET_SECRET_HEX"]))
