"""Regression for the real API response casing observed on 2026-10-05."""
import unittest
from unittest.mock import AsyncMock
from services.voiceprint_gateway.adapters import VolcProvider, voiceprint_id
from services.voiceprint_gateway.models import ProviderUnknown, ServiceError

ID = "a3ee18be-6113-4b9a-8bc1-55fbb9f88f71"
NAME = "listener_owner_01234567890123456789012345678901"

class RegistryResponseTests(unittest.IsolatedAsyncioTestCase):
    async def test_uncertain_create_recovers_only_exact_active_operation_and_confirms_it(self):
        provider = VolcProvider("test-app", "test-key", "test-resource")
        own = {"SpeakID": ID, "SpeakerName": NAME, "Status": 1}
        provider.request = AsyncMock(side_effect=[ProviderUnknown(),
            {"Code": 1000, "AudioEmbedding": [
                {"SpeakID": "b3ee18be-6113-4b9a-8bc1-55fbb9f88f71", "SpeakerName": "other-customer", "Status": 1}, own]},
            {"Code": 1000, "AudioEmbedding": [own]}])
        profile = await provider.register("https://public.example.test/reference.wav", NAME)
        self.assertEqual(profile["id"], ID)
        self.assertEqual([call.args[0] for call in provider.request.call_args_list],
                         ["UpdateVoiceprint", "ListVoiceprint", "GetVoiceprint"])
        self.assertEqual(provider.request.call_args_list[2].args[1], {"SpeakIdList": [ID]})
        self.assertEqual(set(profile), {"id", "speakerName", "accountFingerprint"})

    async def test_uncertain_create_with_missing_deleted_or_ambiguous_match_stays_unknown(self):
        own = {"SpeakID": ID, "SpeakerName": NAME, "Status": 1}
        for rows in ([], [{**own, "SpeakerName": "other-customer"}], [{**own, "Status": 2}],
                     [own, own], [{**own, "SpeakId": "b3ee18be-6113-4b9a-8bc1-55fbb9f88f71"}]):
            with self.subTest(rows=rows):
                provider = VolcProvider("test-app", "test-key", "test-resource")
                provider.request = AsyncMock(side_effect=[ProviderUnknown(), {"Code": 1000, "AudioEmbedding": rows}])
                with self.assertRaises(ProviderUnknown):
                    await provider.register("https://public.example.test/reference.wav", NAME)
                self.assertEqual(provider.request.await_count, 2)

    async def test_recovered_record_must_still_have_same_name_in_get(self):
        provider = VolcProvider("test-app", "test-key", "test-resource")
        own = {"SpeakID": ID, "SpeakerName": NAME, "Status": 1}
        provider.request = AsyncMock(side_effect=[ProviderUnknown(), {"Code": 1000, "AudioEmbedding": [own]},
            {"Code": 1000, "AudioEmbedding": [{**own, "SpeakerName": "other-customer"}]}])
        with self.assertRaises(ProviderUnknown):
            await provider.register("https://public.example.test/reference.wav", NAME)
        self.assertEqual(provider.request.await_count, 3)

    async def test_reconciliation_unavailable_preserves_unknown_and_never_recreates(self):
        provider = VolcProvider("test-app", "test-key", "test-resource")
        provider.request = AsyncMock(side_effect=[ProviderUnknown(), ServiceError(502, "provider_unavailable")])
        with self.assertRaises(ProviderUnknown):
            await provider.register("https://public.example.test/reference.wav", NAME)
        self.assertEqual(provider.request.await_count, 2)

    async def test_definite_create_rejection_does_not_query_or_retry(self):
        provider = VolcProvider("test-app", "test-key", "test-resource")
        provider.request = AsyncMock(side_effect=ServiceError(502, "provider_rejected"))
        with self.assertRaises(ServiceError):
            await provider.register("https://public.example.test/reference.wav", NAME)
        self.assertEqual(provider.request.await_count, 1)

    async def test_live_and_documented_response_shapes_confirm_same_identity(self):
        for field in ("SpeakID", "SpeakId"):
            with self.subTest(field=field):
                provider = VolcProvider("test-app", "test-key", "test-resource")
                provider.request = AsyncMock(side_effect=[
                    {"Code":1000, field:ID},
                    {"Code":1000, "AudioEmbedding":[{field:ID, "SpeakerName":NAME, "Status":1,
                                                     "AudioURL":"private", "Embedding":[1,2], "GroupID":""}]}])
                profile = await provider.register("https://public.example.test/reference.wav", NAME)
                self.assertEqual(profile["id"], ID)
                self.assertEqual(set(profile), {"id", "speakerName", "accountFingerprint"})
                self.assertEqual(provider.request.call_args_list[1].args,
                                 ("GetVoiceprint", {"SpeakIdList":[ID]}))

    async def test_ambiguous_identity_never_binds_or_retries_creation(self):
        provider = VolcProvider("test-app", "test-key", "test-resource")
        provider.request = AsyncMock(return_value={"Code":1000, "SpeakID":ID,
                                                  "SpeakId":"b3ee18be-6113-4b9a-8bc1-55fbb9f88f71"})
        with self.assertRaises(ProviderUnknown):
            await provider.register("https://public.example.test/reference.wav", NAME)
        self.assertEqual(provider.request.await_count, 1)

    async def test_another_registered_name_never_binds_customer(self):
        provider = VolcProvider("test-app", "test-key", "test-resource")
        provider.request = AsyncMock(side_effect=[{"Code":1000, "SpeakID":ID},
              {"Code":1000, "AudioEmbedding":[{"SpeakID":ID, "SpeakerName":"other-customer", "Status":1}]}])
        with self.assertRaises(ProviderUnknown):
            await provider.register("https://public.example.test/reference.wav", NAME)

    def test_missing_or_invalid_ids_are_rejected(self):
        for response in ({}, {"SpeakID":"bad"}, {"SpeakId":""}):
            with self.assertRaises((ValueError, TypeError, AttributeError)):
                voiceprint_id(response)

if __name__ == "__main__": unittest.main()
