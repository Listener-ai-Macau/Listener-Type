"""Listener offline text translation worker (one request, then exit).

The host supplies a verified private embedded runtime and data-only model paths.
No provider, network, user site-packages, microphone or clipboard is accessed.
"""
import json
import os
from pathlib import Path
import re
import sys
sys.dont_write_bytecode = True


def translation_sentences(value):
    """Keep ordinary titles/time abbreviations attached to their continuation.

    A real next sentence still needs a separate model input; joining short
    sentences can make these sentence models omit an earlier instruction.
    The returned units retain every source character without rewriting text.
    """
    temporal_words = {
        'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday',
        'utc', 'gmt',
    }
    units = []
    start = 0
    for match in re.finditer(r'(?<=[。！？!?])|(?<=\.)(?=\s)', value):
        end = match.start()
        if value[end - 1] == '.':
            before, after = value[:end], value[end:]
            if re.search(r'\b(?:Mr|Mrs|Ms|Dr|Prof)\.$', before, re.IGNORECASE) and after.strip():
                continue
            if re.search(r'\b[ap]\.m\.$', before, re.IGNORECASE):
                following = re.match(r'\s*([^\W\d_]+)', after)
                if following and (following[1][0].islower() or following[1].lower() in temporal_words):
                    continue
                if re.match(r'\s*\(\s*(?:UTC|GMT)(?:\s*[+-]\s*\d|\s*\))', after, re.IGNORECASE):
                    continue
        units.append(value[start:end])
        start = end
    units.append(value[start:])
    return units


def split_numbered_list_prefix(value):
    """Separate only an ASCII numbered list marker with following spacing."""
    match = re.match(r'^([ \t]*[0-9]{1,3}[.)][ \t]+)(?=\S)', value)
    prefix = match.group(1) if match else ''
    return prefix, value[len(prefix):]


def translate(request):
    # Windows' extension discovery can otherwise omit a .pyd at MAX_PATH,
    # reporting a misleading circular import. The customer profile/cache may
    # legitimately be deep or contain non-ASCII names; keep this local to the
    # worker instead of changing machine-wide long-path settings.
    if os.name == 'nt':
        def extended(path):
            path = os.path.abspath(path)
            if path.startswith('\\\\?\\'):
                return path
            return '\\\\?\\UNC\\' + path[2:] if path.startswith('\\\\') else '\\\\?\\' + path
        sys.path[:] = [extended(path) for path in sys.path]
    import ctranslate2
    import sentencepiece

    text = request['text']
    if not isinstance(text, str) or not text.strip() or len(text) > 4000:
        raise ValueError('local_translation_invalid_text')
    source = request['source']
    if source not in {'zh', 'en'}:
        raise ValueError('local_translation_unsupported_pair')
    model = Path(request['model'])
    # Native model readers receive short relative paths. Python's wide-char
    # filesystem bridge owns the customer-specific absolute directory.
    os.chdir(model.parent)
    tokenizer = sentencepiece.SentencePieceProcessor(model_file=Path(request['tokenizer']).name)
    engine = ctranslate2.Translator(model.name, device='cpu', compute_type='int8', inter_threads=1, intra_threads=2)

    def paragraph(value):
        # Do not let the engine's default source limit silently discard a long
        # paragraph. Segment on sentence boundaries, then bound token chunks.
        if not value.strip():
            return value
        # These are sentence translation models, not paragraph/chat models.
        # Even a short two-sentence source can collapse to its last sentence.
        # Keep every sentence as a separate unit regardless of token count.
        sentences = translation_sentences(value)
        chunks = []
        for sentence in sentences:
            tokens = tokenizer.encode(sentence, out_type=str)
            if not tokens:
                continue
            while len(tokens) > 320:
                chunks.append(tokens[:320])
                tokens = tokens[320:]
            if tokens:
                # Translate complete sentences individually. Combining many
                # repetitive sentences lets a small model collapse repetitions
                # and omit a distinct last instruction, despite no token limit.
                chunks.append(tokens)
        output = []
        for chunk in chunks:
            # Keep several complete hypotheses in the search. The narrower
            # search can omit an event and its daypart in an ordinary request.
            # This does not certify semantic accuracy; the result remains editable.
            item = engine.translate_batch([chunk], beam_size=8, max_input_length=0,
                                          max_decoding_length=1024, return_scores=False)[0].hypotheses[0]
            if len(item) >= 1024:
                raise ValueError('local_translation_output_limit')
            decoded = tokenizer.decode(item).replace('\u2581', ' ').strip()
            if not decoded:
                raise ValueError('local_translation_empty_output')
            output.append(decoded)
        return ' '.join(output)

    # Preserve paragraph boundaries, including empty lines, without uploading
    # any draft or recording. Ordinary ASCII underscores are left intact.
    output = []
    for value in re.split(r'(\r\n|\r|\n)', text):
        if not value.strip():
            output.append(value)
            continue
        # A sentence model can hallucinate unrelated phrases when a list
        # number such as "1." is submitted as a sentence by itself. Keep
        # actual line-leading list markers outside the model; decimals like
        # "1.5" do not match because the marker requires following space.
        prefix, body = split_numbered_list_prefix(value)
        translated, _ = preserve_explicit_years(body, paragraph(body), source)
        translated, _ = preserve_explicit_times(body, translated, source)
        output.append(prefix + body[:len(body) - len(body.lstrip())] + translated + body[len(body.rstrip()):])
    return ''.join(output)


if __name__ == '__main__':
    try:
        raw = sys.stdin.buffer.read(24_001)
        if len(raw) > 24_000:
            raise ValueError('local_translation_invalid_text')
        result = translate(json.loads(raw.decode('utf8')))
        sys.stdout.buffer.write(json.dumps({'text': result}, ensure_ascii=False).encode('utf8'))
        sys.stdout.buffer.flush()
    except Exception as error:
        # No request text or model contents are printed in failure diagnostics.
        known = str(error)
        if known not in {'local_translation_invalid_text', 'local_translation_unsupported_pair',
                         'local_translation_output_limit', 'local_translation_empty_output',
                         'local_translation_date_fidelity', 'local_translation_time_fidelity'}:
            known = 'local_translation_runtime_failed'
        sys.stdout.buffer.write(json.dumps({'error': known}).encode('utf8'))
        sys.stdout.buffer.flush()
        sys.stderr.write(type(error).__name__ + ': ' + str(error)[:600])
        sys.exit(1)
