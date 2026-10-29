"""Optional CPU translation, one private request, then release the owned server.

The host injects the shared sentence/layout/date helpers and verified paths.
Text travels only through stdin and an authenticated, PID-checked loopback port.
The Windows host owns a kill-on-close Job containing this worker and server.
"""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import secrets
import socket
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

# Avoid a fixed multi-gigabyte admission gate: model mappings and available
# memory vary across machines. Stop the owned worker if the system actually
# reaches a critically low reserve; never switch to a cloud service.
_RESERVE = 512 * 1024**2
_CONTEXT = 8192
_OUTPUT = 4096


class _MemoryStatus(ctypes.Structure):
    _fields_ = [('length', wintypes.DWORD), ('load', wintypes.DWORD)] + [
        (name, ctypes.c_ulonglong) for name in
        ['totalPhys', 'availPhys', 'totalPage', 'availPage', 'totalVirtual', 'availVirtual', 'extended']]


class _TcpOwner(ctypes.Structure):
    _fields_ = [(name, wintypes.DWORD) for name in ['state', 'localAddr', 'localPort', 'remoteAddr', 'remotePort', 'pid']]


def _available_memory():
    value = _MemoryStatus()
    value.length = ctypes.sizeof(value)
    api = ctypes.WinDLL('kernel32', use_last_error=True).GlobalMemoryStatusEx
    api.argtypes = [ctypes.POINTER(_MemoryStatus)]
    api.restype = wintypes.BOOL
    if not api(ctypes.byref(value)):
        raise ValueError('local_translation_runtime_failed')
    return value.availPhys


def _owns_listener(pid, port):
    api = ctypes.WinDLL('iphlpapi', use_last_error=True).GetExtendedTcpTable
    api.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.DWORD), wintypes.BOOL,
                    wintypes.DWORD, ctypes.c_int, wintypes.DWORD]
    api.restype = wintypes.DWORD
    size = wintypes.DWORD()
    result = api(None, ctypes.byref(size), False, socket.AF_INET, 5, 0)
    if result != 122 or not 4 <= size.value <= 1024 * 1024:
        return False
    buffer = ctypes.create_string_buffer(size.value)
    if api(buffer, ctypes.byref(size), False, socket.AF_INET, 5, 0):
        return False
    count = wintypes.DWORD.from_buffer(buffer).value
    if 4 + count * ctypes.sizeof(_TcpOwner) > size.value:
        return False
    for index in range(count):
        row = _TcpOwner.from_buffer(buffer, 4 + index * ctypes.sizeof(_TcpOwner))
        if (row.pid == pid and row.state == 2 and socket.ntohs(row.localPort & 65535) == port
                and socket.inet_ntoa(struct.pack('<I', row.localAddr)) == '127.0.0.1'):
            return True
    return False


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        return None


def _translate_enhanced(request):
    if os.name != 'nt':
        raise ValueError('local_translation_platform_unavailable')
    text, source, target = request['text'], request['source'], request['target']
    if not isinstance(text, str) or not text.strip() or len(text) > 4000:
        raise ValueError('local_translation_invalid_text')
    if (source, target) not in {('zh', 'en'), ('en', 'zh')}:
        raise ValueError('local_translation_unsupported_pair')
    runtime, model = Path(request['runtime']), Path(request['model'])
    if not (runtime / 'llama-server.exe').is_file() or not model.is_file() or (runtime / 'ggml-vulkan.dll').exists():
        raise ValueError('local_translation_not_ready')
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    key, alias = secrets.token_urlsafe(24), 'listener-' + secrets.token_hex(12)
    environment = {name: value for name, value in os.environ.items() if name.upper() in {
        'SYSTEMROOT', 'WINDIR', 'PATH', 'TEMP', 'TMP', 'USERPROFILE', 'APPDATA',
        'LOCALAPPDATA', 'SYSTEMDRIVE', 'PROGRAMDATA', 'COMSPEC'}}
    environment.update(LLAMA_API_KEY=key, OMP_NUM_THREADS='2', OPENBLAS_NUM_THREADS='2')
    command = [str(runtime / 'llama-server.exe'), '--model', os.path.relpath(model, runtime),
               '--host', '127.0.0.1', '--port', str(port), '--alias', alias,
               '--device', 'none', '--gpu-layers', '0', '--ctx-size', str(_CONTEXT),
               '--parallel', '1', '--fit', 'off', '--predict', str(_OUTPUT),
               '--threads', '2', '--threads-batch', '2', '--batch-size', '64',
               '--ubatch-size', '64', '--no-repack', '--cache-ram', '0',
               '--no-context-shift', '--no-ui', '--no-warmup', '--load-mode', 'mmap',
               '--no-op-offload', '--no-kv-offload', '--log-disable']
    child = subprocess.Popen(command, cwd=runtime, env=environment, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             creationflags=subprocess.CREATE_NO_WINDOW)
    stop = threading.Event()
    guard_error = []

    def guard():
        while not stop.wait(.1) and child.poll() is None:
            try:
                if _available_memory() < _RESERVE:
                    guard_error.append('local_translation_memory_low')
                    child.terminate()
                    return
            except Exception:
                guard_error.append('local_translation_runtime_failed')
                if child.poll() is None:
                    child.terminate()
                return

    monitor = threading.Thread(target=guard, daemon=True)
    monitor.start()
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())
    base_url = f'http://127.0.0.1:{port}'

    def call(path, payload=None, timeout=3):
        if child.poll() is not None or not _owns_listener(child.pid, port):
            raise ValueError(guard_error[0] if guard_error else 'local_translation_runtime_failed')
        data = None if payload is None else json.dumps(payload, ensure_ascii=False).encode('utf8')
        req = urllib.request.Request(base_url + path, data=data,
                                     headers={'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'})
        with opener.open(req, timeout=timeout) as response:
            raw = response.read(512001)
            if len(raw) > 512000:
                raise ValueError('local_translation_output_limit')
            return json.loads(raw.decode('utf8'))

    try:
        deadline = time.monotonic() + 90
        while True:
            if child.poll() is not None:
                raise ValueError(guard_error[0] if guard_error else 'local_translation_runtime_failed')
            try:
                if _owns_listener(child.pid, port) and call('/health', timeout=1).get('status') == 'ok':
                    break
            except (urllib.error.URLError, TimeoutError, OSError):
                pass
            if time.monotonic() >= deadline:
                raise ValueError('local_translation_timeout')
            time.sleep(.1)
        if not any(value['id'] == alias for value in call('/v1/models')['data']):
            raise ValueError('local_translation_runtime_failed')
        outputs, cache, restored = [], {}, 0
        for value, count in translation_units(text, translation_sentences):
            if not value.strip():
                outputs.extend([value] * count)
                continue
            if value not in cache:
                language = '英语' if target == 'en' else '中文'
                messages = [{'role': 'user', 'content': f'将以下文本翻译为{language}，注意只需要输出翻译后的结果，不要额外解释：\n\n{value}'}]
                prompt = call('/apply-template', {'messages': messages, 'add_generation_prompt': True})['prompt']
                tokens = call('/tokenize', {'content': prompt, 'add_special': False, 'parse_special': True})['tokens']
                if len(tokens) + _OUTPUT > _CONTEXT:
                    raise ValueError('local_translation_context_limit')
                reply = call('/v1/chat/completions', {'model': alias, 'messages': messages, 'stream': False,
                             'temperature': .7, 'top_p': .6, 'top_k': 20, 'repeat_penalty': 1.05,
                             'seed': 42, 'max_tokens': _OUTPUT, 'cache_prompt': False}, timeout=180)
                choice = reply['choices'][0]
                message = choice['message']
                if choice.get('finish_reason') != 'stop':
                    raise ValueError('local_translation_output_limit')
                if message.get('reasoning_content') or not (message.get('content') or '').strip():
                    raise ValueError('local_translation_empty_output')
                translated, changes = preserve_layout_and_years(value, message['content'], source)
                require_signed_numbers(value, translated)
                require_no_invented_dollar(value, translated, source)
                restored += len(changes)
                cache[value] = translated
            outputs.extend([cache[value]] * count)
        if guard_error:
            raise ValueError(guard_error[0])
        return {'text': assemble_translations(outputs), 'sourceDateYearsRestored': restored}
    finally:
        stop.set()
        if child.poll() is None:
            child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
        monitor.join(timeout=2)


def enhanced_main():
    try:
        raw = sys.stdin.buffer.read(24001)
        if len(raw) > 24000:
            raise ValueError('local_translation_invalid_text')
        result = _translate_enhanced(json.loads(raw.decode('utf8')))
        sys.stdout.buffer.write(json.dumps(result, ensure_ascii=False).encode('utf8'))
        sys.stdout.buffer.flush()
    except Exception as error:
        code = str(error) if isinstance(error, ValueError) and str(error).startswith('local_translation_') else 'local_translation_runtime_failed'
        sys.stdout.buffer.write(json.dumps({'error': code}).encode('utf8'))
        sys.stdout.buffer.flush()
        sys.exit(1)
