"""Keep unambiguous clock facts from the source, without inferring a daypart.

This checks recognized clock points/ranges, not general translation meaning.
Matching requires the same hours/minutes and point/range shape. Conflicting or
missing anchors fail rather than guessing which event a time belongs to.
"""
from collections import Counter, defaultdict
import re


class TimeFidelityError(ValueError):
    pass


_NUMBER = r'(?:[0-9]{1,2}|[一二两三四五]十[一二三四五六七八九]?|十[一二三四五六七八九]?|[零〇一二两三四五六七八九])'
_DAYPART = r'(?:凌晨|清晨|早上|上午|中午|下午|傍晚|晚上|晚间)'
_EN_PERIOD = r'(?:[ap]\.?\s*m\.?|in\s+the\s+(?:morning|afternoon|evening)|at\s+night)'
_CLOCK_CONTEXT = re.compile(r'\b(?:meet|meeting|call|arrive|leave|start|end|ends|appointment|schedule|deadline|today|tomorrow|tonight|monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b', re.I)
_QUANTITY_UNIT = re.compile(r'^\s*(?:dollars?|USD|EUR|samples?|items?|pieces?|percent|km|miles?|kg|kilograms?|minutes?|hours?|days?|weeks?|years?)\b|^\s*[%$€]', re.I)


def _zh_clock(prefix):
    return (rf'(?<!\d)(?P<{prefix}period>{_DAYPART})?\s*(?P<{prefix}hour>{_NUMBER})\s*(?::(?P<{prefix}colonminute>\d{{2}})|[点时]'
            rf'(?:(?P<{prefix}half>半)|\s*(?P<{prefix}minute>{_NUMBER})\s*分?)?'
            r')(?![零〇一二两三四五六七八九十0-9小时])')


def _en_clock(prefix):
    return (rf'(?P<{prefix}hour>\d{{1,2}})(?::(?P<{prefix}minute>\d{{2}})|\.(?P<{prefix}dotminute>\d{{2}})(?=\s*{_EN_PERIOD}))?'
            rf'(?:\s*(?P<{prefix}period>{_EN_PERIOD})|\s*(?P<{prefix}oclock>o[’\x27]clock))?')


_ZH_RANGE = re.compile(_zh_clock('a') + r'\s*(?:到|至|[—–~～-])\s*' + _zh_clock('b'))
_ZH_POINT = re.compile(r'(?P<relation>不早于|不晚于|最迟在?)?' + _zh_clock('a') +
                       r'\s*(?P<suffix>以前|之前|以后|之后|前|后)?')
_EN_RANGE = re.compile(r'(?<![\w:.])(?:(?:between|from)\s+)?' + _en_clock('a') +
                       r'\s*(?:to|and|[—–~～-])\s*' + _en_clock('b') + r'(?![\w:]|\.\d)', re.I)
_EN_POINT = re.compile(r'(?<![\w:.])(?:(?P<relation>no\s+(?:earlier|later)\s+than|not\s+(?:before|after)|before|after|by|at|to)\s+)?' +
                       _en_clock('a') + r'(?![\w:]|\.\d)', re.I)


def _negated_clause(text, start, end, language):
    # A negated verb can change the meaning of a nearby before/after phrase.
    # Do not silently rewrite that scope or drop the negation while repairing
    # a clock. Explicit "not before" / 不早于 are part of the clock relation.
    clause = re.split(r'[。！？!?.,，;；\r\n]', text[:start])[-1] + re.split(r'[。！？!?.,，;；\r\n]', text[end:])[0]
    return bool(re.search(r'不要|不能|不得|禁止|不许|不|未|无|没有', clause) if language == 'zh'
                else re.search(r"\b(?:not|no|never|cannot|can[’']t|don[’']t|mustn[’']t)\b", clause, re.I))


def _number(value):
    if value.isascii() and value.isdigit():
        return int(value)
    digits = {'零': 0, '〇': 0, '一': 1, '二': 2, '两': 2, '三': 3, '四': 4,
              '五': 5, '六': 6, '七': 7, '八': 8, '九': 9}
    if value in digits:
        return digits[value]
    if value.count('十') == 1:
        left, right = value.split('十')
        if (not left or left in digits) and (not right or right in digits):
            return (digits[left] if left else 1) * 10 + (digits[right] if right else 0)
    raise ValueError('unsupported clock numeral')


def _clock(match, prefix):
    values = match.groupdict()
    hour = _number(values[prefix + 'hour'])
    minute_value = values.get(prefix + 'minute') or values.get(prefix + 'colonminute') or values.get(prefix + 'dotminute')
    minute = 30 if values.get(prefix + 'half') else _number(minute_value) if minute_value else 0
    if not 0 <= hour <= 23 or not 0 <= minute <= 59:
        raise ValueError('invalid clock')
    period = values.get(prefix + 'period')
    if period:
        lower = period.lower()
        if lower in {'凌晨', '清晨', '早上', '上午'} or lower.startswith('a') and lower != 'at night' or lower == 'in the morning':
            period = 'am'
        elif lower == '中午':
            # Noon plus another hour is linguistically ambiguous; do not infer.
            if hour != 12:
                raise ValueError('ambiguous noon')
            period = 'pm'
        else:
            period = 'pm'
        if not (1 <= hour <= 12 or hour > 12 and period == 'pm' or hour == 0 and period == 'am'):
            raise ValueError('invalid daypart clock')
    elif hour == 0 or hour > 12:
        period = 'am' if hour == 0 else 'pm'
    return {'hour': hour % 12, 'minute': minute, 'period': period, 'rawHour': hour}


def _mentions(text, language, target_anchors=False):
    found = []
    for shape, pattern in [('range', _ZH_RANGE if language == 'zh' else _EN_RANGE),
                           ('point', _ZH_POINT if language == 'zh' else _EN_POINT)]:
        for match in pattern.finditer(text):
            if any(match.start() < old['end'] and match.end() > old['start'] for old in found):
                continue
            try:
                clocks = [_clock(match, 'a')]
                if language == 'zh' and shape == 'point' and not target_anchors and not any(match.groupdict().get('a' + field)
                        for field in ('period', 'minute', 'colonminute', 'half')) and not match.groupdict().get('suffix') and not match.groupdict().get('relation'):
                    # Bare "十点" can also mean ten points in a list. A range,
                    # daypart, minute, relation or explicit 时 is a clock anchor.
                    if '时' not in match[0]:
                        continue
                if shape == 'range':
                    clocks.append(_clock(match, 'b'))
                    a, b = clocks
                    # A single written qualifier scopes an increasing range.
                    # Crossing midnight needs separate explicit qualifiers.
                    if bool(a['period']) != bool(b['period']):
                        if a['rawHour'] > b['rawHour']:
                            raise ValueError('ambiguous range daypart')
                        a['period'] = b['period'] = a['period'] or b['period']
                if language == 'en' and not any(match.groupdict().get(prefix + field)
                        for prefix in ('a', 'b') if prefix == 'a' or shape == 'range'
                        for field in ('minute', 'dotminute', 'period', 'oclock')):
                    # Bare numbers can be quantities. "at 3" is a clock anchor;
                    # a bare "between 10 and 11" needs the source-side match.
                    if shape == 'range' and not target_anchors:
                        continue
                    if shape != 'range' and (match.groupdict().get('relation') or '').lower() not in {'at', 'before', 'after', 'by'}:
                        continue
                    if shape == 'point':
                        following = text[match.end():]
                        context = re.split(r'[.!?;\n]', text[:match.start()])[-1] + ' ' + following[:35]
                        if _QUANTITY_UNIT.match(following) or not _CLOCK_CONTEXT.search(context):
                            continue  # "budget at 12 dollars" is not a clock.
                relation = (match.groupdict().get('relation') or '').lower()
                suffix = match.groupdict().get('suffix') or ''
                if language == 'zh':
                    relation = 'not_before' if relation == '不早于' else 'by' if relation else 'before' if suffix in {'以前', '之前', '前'} else 'after' if suffix else ''
                elif re.fullmatch(r'no\s+earlier\s+than|not\s+before', relation):
                    relation = 'not_before'
                elif re.fullmatch(r'no\s+later\s+than|not\s+after', relation):
                    relation = 'by'
                elif relation in {'at', 'to'}:
                    relation = ''
                found.append({'start': match.start(), 'end': match.end(), 'clocks': clocks,
                              'shape': shape, 'relation': relation,
                              'negated': _negated_clause(text, match.start(), match.end(), language),
                              'preposition': (match.groupdict().get('relation') or '').lower()})
            except ValueError:
                continue
    return sorted(found, key=lambda value: value['start'])


def _key(value):
    return value['shape'], tuple((clock['hour'], clock['minute']) for clock in value['clocks'])


def _facts(value):
    # Chinese 前/之前 commonly renders either an English "before" or "by"
    # deadline. Preserve that translation; this is not an inclusivity proof.
    relation = 'before' if value['relation'] == 'by' else value['relation']
    return tuple(clock['period'] for clock in value['clocks']), relation, value['negated']


def _render(source, target, language):
    def clock(value):
        hour = value['hour'] or 12
        minute = f":{value['minute']:02d}" if value['minute'] else ''
        if language == 'en':
            period = f" {value['period'][0]}.m." if value['period'] else ''
            return f'{hour}{minute}{period}'
        period = {'am': '上午', 'pm': '下午', None: ''}[value['period']]
        return f"{period}{hour}点" + (f"{value['minute']:02d}分" if value['minute'] else '')

    rendered = [clock(value) for value in source['clocks']]
    if source['shape'] == 'range':
        return 'between ' + ' and '.join(rendered) if language == 'en' else '到'.join(rendered)
    relation = source['relation']
    if language == 'en':
        prep = ('no earlier than' if relation == 'not_before' else relation) or (target['preposition'] if target['preposition'] in {'at', 'to'} else '')
        return (prep + ' ' if prep else '') + rendered[0]
    if relation in {'by', 'not_before'}:
        return ('不晚于' if relation == 'by' else '不早于') + rendered[0]
    return rendered[0] + {'before': '之前', 'after': '之后', '': ''}[relation]


def preserve_explicit_times(source, translated, source_language):
    if source_language not in {'zh', 'en'}:
        return translated, []
    required = _mentions(source, source_language)
    if not required:
        return translated, []
    language = 'en' if source_language == 'zh' else 'zh'
    targets = _mentions(translated, language, target_anchors=True)
    groups, anchors = defaultdict(list), defaultdict(list)
    for value in required:
        groups[_key(value)].append(value)
    for value in targets:
        anchors[_key(value)].append(value)
    if set(anchors) - set(groups):
        raise TimeFidelityError('local_translation_time_fidelity')
    edits = []
    for key, facts in groups.items():
        candidates = anchors.get(key, [])
        if len(candidates) != len(facts):
            raise TimeFidelityError('local_translation_time_fidelity')
        want = Counter(_facts(value) for value in facts)
        have = Counter(_facts(value) for value in candidates)
        if want == have:
            continue
        if len(want) != 1:
            raise TimeFidelityError('local_translation_time_fidelity')
        for candidate in candidates:
            if _facts(candidate) == _facts(facts[0]):
                continue
            if candidate['negated'] != facts[0]['negated'] or (candidate['negated'] and _facts(candidate)[1] != _facts(facts[0])[1]):
                raise TimeFidelityError('local_translation_time_fidelity')
            replacement = _render(facts[0], candidate, language)
            end = candidate['end']
            following = translated[end:].lstrip()
            if language == 'en' and translated[end - 1] == '.' and not replacement.endswith('.') and (not following or following[0].isupper()):
                end -= 1  # Keep a sentence terminator when removing a.m./p.m.
            edits.append({'start': candidate['start'], 'end': end, 'replacement': replacement})
    result = translated
    for edit in sorted(edits, key=lambda value: value['start'], reverse=True):
        result = result[:edit['start']] + edit['replacement'] + result[edit['end']:]
    # Reparse the actual returned text, including span edits and new separators.
    actual = Counter((_key(value), _facts(value)) for value in _mentions(result, language, target_anchors=True))
    expected = Counter((_key(value), _facts(value)) for value in required)
    if any(actual[key] < count for key, count in expected.items()):
        raise TimeFidelityError('local_translation_time_fidelity')
    return result, edits
