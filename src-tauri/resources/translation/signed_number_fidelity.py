"""Reject unverified loss of an explicit negative numeric sign.

This is a bounded check, not a general meaning or arithmetic validator.
"""
from collections import Counter
from decimal import Decimal
import re

_NUMBER = r'[0-9]+(?:\.[0-9]+)?'
_END = r'(?![0-9]|[,.][0-9])'
# Do not mistake the hyphens inside ISO dates or identifiers for signs.
_SIGNED = re.compile(
    r'(?<![A-Za-z0-9_.])(?:[-−]\s*|(?:minus|negative)\s+|负\s*|負\s*|零下\s*)'
    r'(?:[$€£¥￥]\s*)?(' + _NUMBER + r')' + _END,
    re.IGNORECASE,
)
_BELOW_ZERO = re.compile(
    r'(?<![A-Za-z0-9_.])(' + _NUMBER + r')' + _END
    + r'\s*(?:degrees?|°\s*[cf]?)\s+below\s+zero\b',
    re.IGNORECASE,
)
_WORDS = ('zero', 'one', 'two', 'three', 'four', 'five', 'six', 'seven',
          'eight', 'nine', 'ten', 'eleven', 'twelve', 'thirteen', 'fourteen',
          'fifteen', 'sixteen', 'seventeen', 'eighteen', 'nineteen', 'twenty')
_WORD_BELOW_ZERO = re.compile(
    r'(?<![A-Za-z-])(' + '|'.join(_WORDS) + r')\s+degrees?\s+below\s+zero\b',
    re.IGNORECASE,
)


def _negative_numbers(text):
    matches = list(_SIGNED.finditer(text))
    spans = [match.span(1) for match in matches]
    for match in _BELOW_ZERO.finditer(text):
        if match.span(1) not in spans:
            matches.append(match)
    result = Counter(Decimal(match.group(1)) for match in matches)
    result.update(Decimal(_WORDS.index(match.group(1).lower()))
                  for match in _WORD_BELOW_ZERO.finditer(text))
    return result


def require_signed_numbers(source, translated):
    required = _negative_numbers(source)
    if not required:
        return
    actual = _negative_numbers(translated)
    if any(actual[value] < count for value, count in required.items()):
        raise ValueError('local_translation_signed_number_fidelity')
