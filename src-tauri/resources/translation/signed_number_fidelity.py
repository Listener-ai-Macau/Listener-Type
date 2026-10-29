"""Reject unverified loss of an explicit negative numeric sign.

This is a bounded check, not a general meaning or arithmetic validator.
"""
from collections import Counter
from decimal import Decimal
import re

_NUMBER = r'[0-9]+(?:\.[0-9]+)?'
_END = r'(?![0-9,]|\.[0-9])'
# Do not mistake the hyphens inside ISO dates or identifiers for signs.
_SOURCE = re.compile(r'(?<![A-Za-z0-9_.])[-−](' + _NUMBER + r')' + _END)
_TARGET = re.compile(
    r'(?<![A-Za-z0-9_.])(?:[-−]\s*|(?:minus|negative)\s+|负\s*|負\s*|零下\s*)('
    + _NUMBER + r')' + _END,
    re.IGNORECASE,
)


def require_signed_numbers(source, translated):
    required = Counter(Decimal(value) for value in _SOURCE.findall(source))
    if not required:
        return
    actual = Counter(Decimal(value) for value in _TARGET.findall(translated))
    if any(actual[value] < count for value, count in required.items()):
        raise ValueError('local_translation_signed_number_fidelity')
