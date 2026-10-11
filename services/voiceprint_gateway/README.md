# Listener voiceprint gateway candidate

This is an optional companion service for subscription-included **ASR** and
automatic cloud voiceprint enrollment. Email login, payments, and subscription
administration belong to the account service being developed separately.
Local dictation, local wake enrollment, and ordinary direct BYOK dictation remain
usable without this service. This candidate is not deployed or release accepted.

## Account integration contract

The account service issues short-lived RS256 access tokens with a configured
issuer, audience `listener-cloud`, and HTTPS JWKS endpoint. Tokens require
`sub` (stable customer ID, not mutable email), `iat`, `exp` (at most 15 minutes),
`iss`, `aud`, and `email_verified: true`. Signed entitlements use this shape:

```json
{
  "listener": {
    "asrEnabled": true,
    "billingPeriod": "2026-10",
    "includedAsrSeconds": 3600
  }
}
```

These are example entitlements, not a chosen commercial plan. The account service
must keep a stable billing period identifier and refresh/revoke these grants when
subscriptions change. API request bodies cannot grant entitlement. Revocation is
bounded by token expiry; a production account service may supply an additional
revocation adapter. No development-token bypass exists in the production factory.

After email login, the desktop account UI calls `setCloudAccountSession(token)`.
The native layer validates it via `/v1/account` and saves it in the existing
chunked OS credential vault. Tokens never appear in settings or credentials
snapshot IPC. On logout call `clearCloudAccountSession()`. Changing subjects or
logging out invalidates native managed audio, final frames, and recovery replays.
Token refresh for the same customer preserves an in-progress enrollment.

The account UI still needs to be built/connected by the account project. Set
`cloudModelRoute` to `managed` or `byok` and use `activeAsrProvider: volcengine` for
this SAUC integration. BYOK model keys stay in the OS vault and go directly to
Volcengine; the gateway never receives them. Managed ASR uses only the email token
and a one-use session ticket. Shared provider keys are server environment values.

## Existing three-sample enrollment

Display cloud enrollment consent before calling
`startCloudVoiceprintEnrollment(route, 'voiceprint-cloud-v1')`. The consent must
explain temporary upload of the three guided wake samples, cloud storage of the
registered voiceprint/sample by the selected model provider, and deletion of the
cloud registration. The existing local-only enrollment button does not opt in.
No fourth prompt, customer audio URL, TOS setup, or global registry browser exists.

The native capture consumer uses the same accepted three-step PCM, after local
phrase/quality checks. It uploads three 3-second, 16 kHz, mono PCM16 samples. A
bounded missing transport tail is padded only after existing local validation.
Local templates remain protected; desktop raw audio is dropped after processing.
The server joins the samples into a 9-second WAV and deletes private temporary
objects after success. Incomplete/unknown operations need the bucket lifecycle
backstop described below. A cloud failure does not erase the local template.

The API flow is:

1. `POST /v1/voiceprints/enrollments`: UUID request ID, `mode`, consent version;
   BYOK additionally supplies SHA-256 of its trimmed App ID.
2. `PUT /v1/voiceprints/enrollments/{id}/samples/{0..2}`: `audio/pcm`, exactly
   96,000 bytes each. Repeated identical samples are idempotent; conflicts fail.
3. Managed registration happens on the server, then a confirmed ID/name/account
   binding is published for that customer. Prior ready binding survives failure.
4. BYOK calls `POST .../{id}/byok-registration-start` once to get a short signed
   HTTPS audio URL and unique name. Its native client registers directly with its
   own provider credentials and posts `.../{id}/byok-confirmation`. Before the
   create, the service persists `registering`; unknown results cannot blindly
   repeat it. BYOK IDs never authorize managed ASR.
5. Status is caller-scoped `GET .../{id}`. Cancel collecting enrollment with
   `DELETE .../{id}`. After an uncertain create, managed and native BYOK adapters
   make one bounded read-only lookup for the unique operation name and confirm
   its active ID/name with Get. Creation is never repeated. Missing, deleted,
   ambiguous or changed records remain unknown; unresolved mutations and restart
   recovery still require the owning account/operator to reconcile them.

Cloud deletion uses `DELETE /v1/voiceprints/binding` for the authenticated active
route. Managed deletion revokes the binding before provider soft deletion. BYOK
provider deletion is performed by the native client with its own keys. Deleting
the local/active cloud voiceprint does not turn off automatic wake. Other BYOK
accounts require their own account selection/deletion UI; this candidate does not
claim a cross-account physical-erasure workflow.

## Managed streaming and quota

`POST /v1/asr/sessions` accepts only a UUID request ID. It checks signed subscription
entitlements, reserves up to 180 seconds atomically, and returns the authenticated
customer's managed profile, WSS URL, and 60-second one-use ticket. Reserved and used
PCM seconds are quota accounting, not a provider invoice or completed payment API.
Concurrent reservations cannot spend the same remaining seconds. Unused expired
tickets release quota; terminal sessions settle once and return unused capacity.

The WebSocket requires both the email bearer token and
`X-Listener-Session-Token`. Wrong customer, expired, used, or revoked profile tickets
fail before connecting to the model. The gateway rewrites the first SAUC frame:
one owned voiceprint, `ssd_mode: 2`, full attributed rows, `enable_nonstream: true`,
PCM16/16 kHz/mono. Caller identity, model keys, audio URLs, and arbitrary voiceprints
cannot override it. Native positive/negative sequences are preserved. The gateway
counts PCM bytes, bounds duration/frame sizes, forwards provider frames, and does
not automatically reconnect upstream. The desktop's strict named-owner projection
remains the final text authority; missing/anonymous evidence remains Pending.

## Operator setup and verification

Run one worker/one persistent SQLite replica for this candidate. Use a persistent
`/data` volume, HTTPS reverse proxy with request-size/timeout limits and WebSocket
support, private TOS bucket with public access blocked, and a lifecycle rule that
expires `enrollment/` objects within one day. Use dedicated storage permissions
and protect backups/environment secrets. Multiple replicas require a shared DB
and operation-lock adapter before deployment; two-customer tests are not a load test.

Fill `.env.example` on the server, never on the desktop. The operator configures
TOS once. Configure desktop `LISTENER_TYPE_CLOUD_BASE_URL` to the deployed HTTPS
origin, preferably at build time. It defaults to empty. No service domain has been
chosen, no real user sample was uploaded, and no shared model credential is bundled.

From the repository root:

```sh
python -m pip install -r services/voiceprint_gateway/requirements.txt
python -m unittest services.voiceprint_gateway.tests.test_gateway -v
uvicorn services.voiceprint_gateway.app:production_app --factory --workers 1 --no-access-log
docker build -f services/voiceprint_gateway/Dockerfile -t listener-voiceprint-gateway .
```

The tests inject fake auth/storage/model adapters but exercise actual HTTP and
WebSocket routes, three-sample assembly, two-customer isolation, duplicate and
concurrent requests, quota, and protocol binding. JWT signature/claims tests use
real RSA keys. Production startup fails without operator/account configuration.

Deployment, account UI/login integration, real TOS/provider registration, and new
owner-only/foreign-only/phone-drama overlapping audio still require acceptance.
This gateway cannot establish that short-audio 1:1 verification separates overlapping
speech. No candidate MSI or installed-app acceptance is produced by these tests.

Official protocol references:

- [SAUC speaker verification](https://docs.volcengine.com/docs/DoubaoVoice/speaker-separation?lang=zh)
- [ASR voiceprint registry](https://docs.volcengine.com/docs/DoubaoVoice/voiceprint-registration-management-api?lang=zh)
- [TOS Python SDK](https://github.com/volcengine/ve-tos-python-sdk)
- [JWT verification](https://pyjwt.readthedocs.io/en/stable/usage.html)
- [FastAPI WebSocket endpoints](https://fastapi.tiangolo.com/advanced/websockets/)
