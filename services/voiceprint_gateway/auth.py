import asyncio
import hashlib
from urllib.parse import urlsplit

import jwt

from .models import Principal, ServiceError


def secure_url(value: str) -> str:
    url = urlsplit(value)
    if (url.scheme != "https" or not url.hostname or url.username or url.password
            or url.query or url.fragment or url.hostname in {"localhost", "127.0.0.1", "::1"}):
        raise ValueError("A configured public HTTPS URL is required")
    return value.rstrip("/")


class JwtAuthenticator:
    def __init__(self, issuer: str, audience: str, jwks_url: str):
        secure_url(issuer)
        self.issuer, self.audience = issuer, audience
        if not audience:
            raise ValueError("JWT audience is required")
        self.keys = jwt.PyJWKClient(secure_url(jwks_url), timeout=5, lifespan=300)

    def verify(self, authorization: str | None) -> Principal:
        if not authorization or not authorization.startswith("Bearer "):
            raise ServiceError(401, "login_required")
        token = authorization[7:]
        if not token or len(token) > 16384:
            raise ServiceError(401, "invalid_login")
        try:
            key = self.keys.get_signing_key_from_jwt(token).key
            claims = jwt.decode(token, key, algorithms=["RS256"], issuer=self.issuer,
                                audience=self.audience,
                                options={"require": ["exp", "iat", "sub", "iss", "aud"]},
                                leeway=5)
            subject = claims["sub"]
            if (not isinstance(subject, str) or not subject or len(subject) > 256
                    or claims.get("email_verified") is not True
                    or claims["exp"] - claims["iat"] > 900):
                raise ValueError("invalid subject or token lifetime")
            entitlement = claims.get("listener", {})
            if not isinstance(entitlement, dict):
                raise ValueError("invalid entitlement")
            period = entitlement.get("billingPeriod", "")
            seconds = entitlement.get("includedAsrSeconds", 0)
            if not isinstance(period, str) or len(period) > 128:
                raise ValueError("invalid billing period")
            if type(seconds) is not int or not 0 <= seconds <= 10_000_000:
                raise ValueError("invalid quota")
            enabled = entitlement.get("asrEnabled") is True and bool(period)
            customer = hashlib.sha256((self.issuer + "\0" + subject).encode()).hexdigest()
            return Principal(customer, period, seconds, enabled)
        except Exception:
            # No tokens, claims, signed URLs, or provider errors in responses/logs.
            raise ServiceError(401, "invalid_login") from None

    async def authenticate(self, authorization: str | None) -> Principal:
        return await asyncio.to_thread(self.verify, authorization)
