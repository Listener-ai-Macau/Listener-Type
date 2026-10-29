"""Reject a specific ungrounded dollar sign in a generic Chinese 元 amount.

The original may mean different currencies. Never infer a replacement currency.
This is not a general currency or amount-association validator.
"""
import re

_BARE_YUAN = re.compile(r'(?:[0-9]+(?:\.[0-9]+)?[万萬]?|[零〇一二两兩三四五六七八九十百千万萬]+)\s*元(?!素|旦|件|组|組|朝)')
_SOURCE_DOLLAR = re.compile(r'\$|美元|美金|港元|港幣|港币|澳元|澳幣|澳币|加元|加幣|加币|新加坡元|新元|美元|\b(?:USD|HKD|CAD|AUD|SGD|dollars?)\b', re.IGNORECASE)
_TARGET_DOLLAR = re.compile(r'(?<![A-Za-z])(?:US\s*\$|\$|USD(?=\s|[0-9]|$)|(?:US\s+)?dollars?\b)', re.IGNORECASE)


def require_no_invented_dollar(source, translated, source_language):
    if (source_language == 'zh' and _BARE_YUAN.search(source)
            and not _SOURCE_DOLLAR.search(source) and _TARGET_DOLLAR.search(translated)):
        raise ValueError('local_translation_currency_fidelity')
