from dataclasses import dataclass
from typing import Literal
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field


class Input(BaseModel):
    model_config = ConfigDict(extra="forbid")


class EnrollmentInput(Input):
    requestId: UUID
    mode: Literal["managed", "byok"]
    accountFingerprint: str | None = Field(default=None, pattern=r"^[a-f0-9]{64}$")
    consentVersion: Literal["voiceprint-cloud-v1"]


class ByokConfirmation(Input):
    id: UUID
    speakerName: str
    accountFingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")


class SessionInput(Input):
    requestId: UUID


@dataclass(frozen=True)
class Principal:
    # Derived from verified issuer + subject, never an API body/customer query field.
    customer: str
    period: str
    included_seconds: int
    managed_enabled: bool


class ServiceError(Exception):
    def __init__(self, status: int, code: str):
        self.status, self.code = status, code


class ProviderUnknown(Exception):
    """A create/delete may have reached the provider: never blindly repeat it."""
