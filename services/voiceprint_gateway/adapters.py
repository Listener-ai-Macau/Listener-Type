import asyncio
import hashlib
import os
from contextlib import asynccontextmanager
from uuid import UUID

import httpx
from websockets.asyncio.client import connect

from .auth import secure_url
from .models import ProviderUnknown, ServiceError

REGISTRY = "https://openspeech.bytedance.com/api/proxy/invoke/"
ASR = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async"


def voiceprint_id(value: dict) -> str:
    # Live registry uses SpeakID; the documented examples use SpeakId.
    live, documented = value.get("SpeakID"), value.get("SpeakId")
    if live is not None and documented is not None and live != documented:
        raise ValueError("Ambiguous provider identity")
    value = live if live is not None else documented
    if not isinstance(value, str):
        raise ValueError("Missing provider identity")
    return str(UUID(value))


class TosStorage:
    def __init__(self):
        import tos
        self.tos = tos
        self.bucket = os.environ["TOS_BUCKET"]
        self.client = tos.TosClientV2(ak=os.environ["TOS_ACCESS_KEY"], sk=os.environ["TOS_SECRET_KEY"],
                                     endpoint=secure_url(os.environ["TOS_ENDPOINT"]),
                                     region=os.environ["TOS_REGION"], max_retry_count=1,
                                     request_timeout=15, follow_redirect_times=0)

    async def put(self, key: str, audio: bytes):
        await asyncio.to_thread(self.client.put_object, self.bucket, key, content=audio,
                                content_type="audio/wav", acl=self.tos.ACLType.ACL_Private)

    async def get(self, key: str) -> bytes:
        def read():
            return self.client.get_object(self.bucket, key).read()
        return await asyncio.to_thread(read)

    async def url(self, key: str) -> str:
        result = self.client.pre_signed_url(self.tos.HttpMethodType.Http_Method_Get,
                                           self.bucket, key, expires=300)
        if not result.signed_url.startswith("https://"):
            raise ServiceError(503, "storage_configuration_error")
        return result.signed_url

    async def delete(self, key: str):
        await asyncio.to_thread(self.client.delete_object, self.bucket, key)


class VolcProvider:
    def __init__(self, app_id: str, access_key: str, resource_id: str):
        if not all((app_id.strip(), access_key.strip(), resource_id.strip())):
            raise ValueError("Managed provider credentials are required")
        self.fingerprint = hashlib.sha256(app_id.strip().encode()).hexdigest()
        self.app_id, self.access_key, self.resource_id = app_id, access_key, resource_id

    async def request(self, action: str, body: dict, mutation=False) -> dict:
        try:
            async with httpx.AsyncClient(timeout=20, follow_redirects=False, trust_env=False) as client:
                response = await client.post(REGISTRY, params={"Action": action}, json=body,
                                             headers={"X-Api-App-Id": self.app_id,
                                                      "X-Api-Access-Key": self.access_key})
            if response.status_code >= 500 and mutation:
                raise ProviderUnknown()
            if response.status_code != 200:
                raise ServiceError(502, "provider_rejected")
            payload = response.json()
            if payload.get("ResponseMetadata", {}).get("Error"):
                raise ServiceError(502, "provider_rejected")
            result = payload.get("Result", {})
            if result.get("Code") != 1000:
                raise ServiceError(502, "provider_rejected")
            return result
        except (httpx.TransportError, ValueError):
            if mutation:
                raise ProviderUnknown() from None
            raise ServiceError(502, "provider_unavailable") from None

    async def register(self, audio_url: str, name: str) -> dict:
        try:
            result = await self.request("UpdateVoiceprint", {"Action": 0, "AudioUrl": audio_url,
                                                            "SpeakerName": name}, mutation=True)
        except ProviderUnknown:
            # The unique operation name is owned by the enrollment service.
            # A lost response is not permission to create again. Reconcile once
            # using read-only calls, then confirm this exact id/name below.
            try:
                listing = await self.request("ListVoiceprint", {"Limit": 50, "Offset": 0})
                rows = listing.get("AudioEmbedding")
                if not isinstance(rows, list):
                    raise ValueError()
                matches = [row for row in rows if isinstance(row, dict)
                           and row.get("SpeakerName") == name and row.get("Status") == 1]
                if len(matches) != 1:
                    raise ValueError()
                result = matches[0]
            except (KeyError, ValueError, ServiceError):
                raise ProviderUnknown() from None
        try:
            voice_id = voiceprint_id(result)
            confirmed = await self.request("GetVoiceprint", {"SpeakIdList": [voice_id]})
            record = next(row for row in confirmed["AudioEmbedding"]
                          if voiceprint_id(row) == voice_id and row["Status"] == 1)
            if record["SpeakerName"] != name:
                raise ValueError()
        except (KeyError, StopIteration, ValueError, ServiceError):
            raise ProviderUnknown() from None
        return {"id": voice_id, "speakerName": name, "accountFingerprint": self.fingerprint}

    async def delete(self, profile: dict):
        await self.request("UpdateVoiceprint", {"Action": 2, "SpeakId": profile["id"]}, mutation=True)

    @asynccontextmanager
    async def stream(self, session_id: str):
        # No automatic reconnect: replaying audio can duplicate provider billing/output.
        async with connect(ASR, additional_headers={"X-Api-App-Key": self.app_id,
                           "X-Api-Access-Key": self.access_key, "X-Api-Resource-Id": self.resource_id,
                           "X-Api-Connect-Id": session_id}, proxy=None, compression=None,
                           open_timeout=10, max_size=1024 * 1024, max_queue=16) as socket:
            yield socket
