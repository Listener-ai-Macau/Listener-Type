"""Preserve source layout, explicit dates and repeated content around translation.

The model keeps the full context unless consecutive identical sentence units
must be expanded explicitly. No source fact is invented or normalized.
"""
import re

if 'preserve_explicit_years' not in globals():
    from date_fidelity import DateFidelityError, preserve_explicit_years


def translation_units(source, splitter):
    sentences = splitter(source)
    if ''.join(sentences) != source:
        raise ValueError('local_translation_structure_fidelity')
    pieces = []
    pending = ''
    index = 0
    while index < len(sentences):
        end = index + 1
        value = sentences[index]
        if value.strip() and '\n' not in value and '\r' not in value:
            while end < len(sentences) and sentences[end] == value:
                end += 1
        if end - index > 1:
            if pending:
                pieces.append((pending, 1))
                pending = ''
            pieces.append((value, end - index))
        else:
            pending += value
        index = end
    if pending:
        pieces.append((pending, 1))
    if ''.join(value * count for value, count in pieces) != source:
        raise ValueError('local_translation_structure_fidelity')
    return pieces


def preserve_layout_and_years(source, translated, source_language):
    parts = re.split(r'(\r\n|\r|\n)', source)
    target_lines = [line.strip() for line in re.split(r'\r\n|\r|\n', translated) if line.strip()]
    source_lines = [part for part in parts if part.strip()]
    if len(source_lines) != len(target_lines):
        raise ValueError('local_translation_structure_fidelity')
    index = 0
    result = []
    edits = []
    for part in parts:
        if not part.strip():
            result.append(part)
            continue
        value, changes = preserve_explicit_years(part, target_lines[index], source_language)
        leading = part[:len(part) - len(part.lstrip())]
        trailing = part[len(part.rstrip()):]
        result.append(leading + value + trailing)
        edits.extend({**change, 'sourceLine': index} for change in changes)
        index += 1
    return ''.join(result), edits


def assemble_translations(parts, max_bytes=60000):
    result = ''
    size = 0
    for value in parts:
        separator = bool(result and value and not result[-1].isspace() and not value[0].isspace())
        size += len(value.encode('utf8')) + int(separator)
        if size > max_bytes:
            raise ValueError('local_translation_output_limit')
        if separator:
            result += ' '
        result += value
    return result
