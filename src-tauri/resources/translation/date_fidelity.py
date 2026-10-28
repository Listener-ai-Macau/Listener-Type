"""Keep explicit source calendar years when a matching translated date omits them.

Only an unambiguous month/day match can receive a missing source year. Wrong
dates, conflicting years and ambiguous references are rejected, never guessed.
This does not certify the remainder of a model's translation.
"""
from collections import Counter, defaultdict
import datetime
import re


class DateFidelityError(ValueError):
    pass


_MONTH_WORDS = ('Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|'
                'Jul(?:y)?|Aug(?:ust)?|Sep(?:tember)?|Sept|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?')
_MONTHS = {name: index + 1 for index, name in enumerate(
    ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec'])}
_ISO = re.compile(r'(?<!\d)(?P<base>(?P<year>\d{4})-(?P<month>\d{1,2})-(?P<day>\d{1,2}))(?!\d)')
_CHINESE = re.compile(r'(?<!\d)(?:(?P<year>\d{4})年\s*)?(?P<base>(?P<month>\d{1,2})月\s*(?P<day>\d{1,2})日)')
_MONTH_FIRST = re.compile(
    rf'\b(?P<base>(?P<word>{_MONTH_WORDS})\.?\s+(?P<day>\d{{1,2}})(?:st|nd|rd|th)?)(?!\d)'
    r'(?:(?:,\s*|\s+)(?P<year>\d{4})(?!\d))?', re.IGNORECASE)
_DAY_FIRST = re.compile(
    rf'(?<!\d)(?P<base>(?P<day>\d{{1,2}})(?:st|nd|rd|th)?\s+(?P<word>{_MONTH_WORDS})\.?)(?!\w)'
    r'(?:(?:,\s*|\s+)(?P<year>\d{4})(?!\d))?', re.IGNORECASE)


def _dates(text, language):
    patterns = [(_ISO, 'iso')]
    patterns += [(_CHINESE, 'prefix')] if language == 'zh' else [(_MONTH_FIRST, 'comma'), (_DAY_FIRST, 'space')]
    found = []
    for pattern, style in patterns:
        for match in pattern.finditer(text):
            groups = match.groupdict()
            month = _MONTHS[groups['word'][:3].lower()] if groups.get('word') else int(groups['month'])
            day = int(groups['day'])
            year = int(groups['year']) if groups.get('year') else None
            try:
                datetime.date(year if year is not None else 2000, month, day)
            except ValueError:
                continue
            found.append({'start': match.start(), 'end': match.end(), 'month': month, 'day': day,
                          'year': year, 'style': style, 'baseStart': match.start('base'),
                          'baseEnd': match.end('base'), 'text': match[0]})
    result = []
    for value in sorted(found, key=lambda x: (x['start'], -x['end'])):
        if not result or value['start'] >= result[-1]['end']:
            result.append(value)
    return result


def preserve_explicit_years(source, translated, source_language):
    if source_language not in {'zh', 'en'}:
        return translated, []
    facts = defaultdict(list)
    for value in _dates(source, source_language):
        if value['year'] is not None:
            facts[(value['month'], value['day'])].append(value)
    if not facts:
        return translated, []
    target_language = 'en' if source_language == 'zh' else 'zh'
    targets = _dates(translated, target_language)
    edits = []
    for key, required in facts.items():
        anchors = [value for value in targets if (value['month'], value['day']) == key]
        years = {value['year'] for value in required}
        if not anchors or any(value['year'] is not None and value['year'] not in years for value in anchors):
            raise DateFidelityError('local_translation_date_fidelity')
        want = Counter(value['year'] for value in required)
        have = Counter(value['year'] for value in anchors if value['year'] is not None)
        if all(have[year] >= count for year, count in want.items()):
            continue
        if len(years) != 1 or len(anchors) != len(required):
            raise DateFidelityError('local_translation_date_fidelity')
        year = next(iter(years))
        for anchor in anchors:
            if anchor['year'] is not None:
                continue
            if anchor['style'] == 'prefix':
                position, value = anchor['baseStart'], f'{year}年'
            elif anchor['style'] == 'comma':
                position, value = anchor['baseEnd'], f', {year}'
            elif anchor['style'] == 'space':
                position, value = anchor['baseEnd'], f' {year}'
            else:
                raise DateFidelityError('local_translation_date_fidelity')
            edits.append({'position': position, 'insert': value, 'year': year,
                          'month': key[0], 'day': key[1], 'sourceDates': [x['text'] for x in required]})
    result = translated
    for edit in sorted(edits, key=lambda x: x['position'], reverse=True):
        result = result[:edit['position']] + edit['insert'] + result[edit['position']:]
    actual = Counter((x['year'], x['month'], x['day']) for x in _dates(result, target_language) if x['year'] is not None)
    expected = Counter((x['year'], x['month'], x['day']) for values in facts.values() for x in values)
    if any(actual[key] < count for key, count in expected.items()):
        raise DateFidelityError('local_translation_date_fidelity')
    return result, edits
