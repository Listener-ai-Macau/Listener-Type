"""Phase3: clear soft>4500 by extracting dictation + commands include chunks."""
from __future__ import annotations

import importlib.util
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("split", ROOT / "tools" / "_split_embedded_ble.py")
split = importlib.util.module_from_spec(spec)
assert spec.loader
spec.loader.exec_module(split)


def fn_end(text: str, name: str) -> tuple[int, int]:
    m = re.search(rf"(?m)^(pub(\([^)]*\))? )?(async )?fn {re.escape(name)}\b", text)
    if not m:
        m = re.search(rf"(?m)^impl {re.escape(name)}\b", text)
    if not m:
        m = re.search(rf"(?m)^impl Drop for {re.escape(name)}\b", text)
    if not m:
        raise SystemExit(f"not found: {name}")
    brace = text.find("{", m.start())
    end = split.find_matching_brace(text, brace)
    return text.count("\n", 0, m.start()) + 1, text.count("\n", 0, end) + 1


def extract(
    file_path: Path,
    ranges: list[tuple[str, int, int, list[str]]],
) -> None:
    lines = file_path.read_text(encoding="utf-8").splitlines(keepends=True)
    for name, s, e, header in ranges:
        print(f"{file_path.name}: {name} L{s}-{e}")
        print(" ", lines[s - 1][:90].rstrip())
        print(" ", lines[e - 1][:90].rstrip())
        # End may be `}` of last fn, or a section comment if intentionally cut.
        ok_end = (
            lines[e - 1].strip() == "}"
            or lines[e - 1].rstrip().endswith("}")
            or lines[e - 1].strip().startswith("//")
        )
        assert ok_end, f"bad end line for {name}: {lines[e-1]!r}"
    for name, s, e, header in ranges:
        start, end = s - 1, e
        out = file_path.parent / name
        out.write_text("".join(header + lines[start:end]), encoding="utf-8", newline="\n")
        print(f"  wrote {out.name} {sum(1 for _ in out.open(encoding='utf-8'))}")
        before = lines[:start]
        after = lines[end:]
        while before and before[-1].strip() == "":
            before.pop()
        while after and after[0].strip() == "":
            after.pop(0)
        lines = before + ["\n", f'include!("{name}");\n', "\n"] + after
    file_path.write_text("".join(lines), encoding="utf-8", newline="\n")
    print(f"  rewrote {file_path.name} {sum(1 for _ in file_path.open(encoding='utf-8'))}")
    print("  includes", [ln.strip() for ln in lines if "include!" in ln])


def main() -> None:
    # --- dictation ranges from ORIGINAL file ---
    dpath = ROOT / "src-tauri" / "src" / "coordinator" / "dictation.rs"
    dtext = dpath.read_text(encoding="utf-8")
    stream_s, stream_e = fn_end(dtext, "EmbeddedStreamingDictation")
    # start from submit_embedded_audio_notifications through stream_impl end if before EmbeddedStreaming
    sub_s, _ = fn_end(dtext, "submit_embedded_audio_notifications")
    _, sub_e = fn_end(dtext, "submit_embedded_audio_ble_stream_impl")
    sess_s, _ = fn_end(dtext, "handle_pressed_edge")
    _, sess_e = fn_end(dtext, "finish_starting_session")
    wake_s, _ = fn_end(dtext, "EmbeddedAudioDictationSession")
    _, wake_e = fn_end(dtext, "device_processing_final_succeeded")
    dev_s, _ = fn_end(dtext, "apply_and_publish_dictation_event")
    _, dev_e = fn_end(dtext, "request_embedded_ble_capture_cancel")

    print("dictation bounds", {
        "stream": (stream_s, stream_e),
        "submit": (sub_s, sub_e),
        "session": (sess_s, sess_e),
        "wake": (wake_s, wake_e),
        "device": (dev_s, dev_e),
    })

    extract(
        dpath,
        [
            (
                "dictation_embedded_stream.rs",
                stream_s,
                stream_e,
                [
                    "// EmbeddedStreamingDictation actor / PCM submit path.\n",
                    "// Included into `coordinator::dictation` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "dictation_embedded_submit.rs",
                sub_s,
                sub_e,
                [
                    "// Embedded audio / BLE submit entrypoints.\n",
                    "// Included into `coordinator::dictation` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "dictation_session.rs",
                sess_s,
                sess_e,
                [
                    "// Press/release/begin/start recorder session lifecycle.\n",
                    "// Included into `coordinator::dictation` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "dictation_wake_polish.rs",
                wake_s,
                wake_e,
                [
                    "// Wake candidate, speaker gate, streaming polish helpers.\n",
                    "// Included into `coordinator::dictation` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "dictation_device_ai.rs",
                dev_s,
                dev_e,
                [
                    "// Dictation event publish + device AI processing LED helpers.\n",
                    "// Included into `coordinator::dictation` via `include!`.\n",
                    "\n",
                ],
            ),
        ],
    )

    # --- commands ---
    cpath = ROOT / "src-tauri" / "src" / "commands" / "mod.rs"
    ctext = cpath.read_text(encoding="utf-8")
    # marketplace section from comment through last github command before thin ble wrappers
    # find start line of marketplace comment
    mkt_start = None
    for i, line in enumerate(ctext.splitlines(), 1):
        if "marketplace (Phase A)" in line:
            mkt_start = i
            break
    if mkt_start is None:
        raise SystemExit("marketplace section missing")
    # end: last function before get_embedded_ble_runtime_status thin wrapper, or end of github logout
    # Use start of get_embedded_ble_runtime_status - 1
    m = re.search(r"(?m)^pub fn get_embedded_ble_runtime_status\b", ctext)
    if not m:
        raise SystemExit("get_embedded_ble_runtime_status missing")
    mkt_end = ctext.count("\n", 0, m.start())  # line before that fn
    # walk back over blank lines / attrs
    lines = ctext.splitlines(keepends=True)
    end_idx = mkt_end  # 0-based line index of get_embedded... is mkt_end
    # mkt_end as last inclusive line of previous content
    while end_idx > 0 and lines[end_idx - 1].strip() in ("",):
        end_idx -= 1
    # also skip #[tauri::command] belonging to get_embedded
    # find start of get_embedded attrs
    ge_line = ctext.count("\n", 0, m.start()) + 1
    # inclusive end is ge_line-1 but may include #[tauri::command]
    mkt_end_line = ge_line - 1
    while mkt_end_line > 0 and (
        lines[mkt_end_line - 1].strip().startswith("#[")
        or lines[mkt_end_line - 1].strip() == ""
    ):
        mkt_end_line -= 1

    # diagnostics from export_error_log or export_diagnostic_package
    _, diag_start = fn_end(ctext, "export_error_log")
    # wait export_error_log start
    diag_s, _ = fn_end(ctext, "export_error_log")
    # actually start at export_error_log
    # end before marketplace
    diag_e = mkt_start - 1
    while diag_e > 0 and lines[diag_e - 1].strip() == "":
        diag_e -= 1

    style_s = None
    for i, line in enumerate(ctext.splitlines(), 1):
        if "style packs" in line and "────" in line:
            style_s = i
            break
    style_e = None
    for i, line in enumerate(ctext.splitlines(), 1):
        if "style toggles" in line and "────" in line:
            style_e = i - 1
            break
    while style_e and style_e > 0 and lines[style_e - 1].strip() == "":
        style_e -= 1
    # include style toggles through open_system_settings? Keep style packs only for now.
    # expand style to end of delete/export packs - style_e already before toggles
    # better include toggles until check_accessibility
    acc_s, _ = fn_end(ctext, "check_accessibility_permission")
    style_e2 = acc_s - 1
    while style_e2 > 0 and (
        lines[style_e2 - 1].strip().startswith("#[") or lines[style_e2 - 1].strip() == ""
    ):
        style_e2 -= 1

    local_s, _ = fn_end(ctext, "local_asr_get_settings")
    # include foundry through emit_foundry
    _, foundry_e = fn_end(ctext, "emit_foundry_prepare_progress")
    # but need start before #[tauri] on local_asr - walk back
    local_start = local_s
    while local_start > 1 and (
        lines[local_start - 2].strip().startswith("#[")
        or lines[local_start - 2].strip() == ""
    ):
        local_start -= 1

    print(
        "commands bounds",
        {
            "marketplace": (mkt_start, mkt_end_line),
            "diag": (diag_s, diag_e),
            "style": (style_s, style_e2),
            "local_asr": (local_start, foundry_e),
        },
    )

    extract(
        cpath,
        [
            (
                "marketplace.rs",
                mkt_start,
                mkt_end_line,
                [
                    "// Marketplace + GitHub OAuth command surface.\n",
                    "// Included into `commands` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "diagnostics_export.rs",
                diag_s,
                diag_e,
                [
                    "// Diagnostic package / error-log export commands.\n",
                    "// Included into `commands` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "local_asr_commands.rs",
                local_start,
                foundry_e,
                [
                    "// Local / Foundry ASR settings and prepare commands.\n",
                    "// Included into `commands` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "style_pack_commands.rs",
                style_s,
                style_e2,
                [
                    "// Style pack list/create/import and polish-mode toggles.\n",
                    "// Included into `commands` via `include!`.\n",
                    "\n",
                ],
            ),
        ],
    )

    # budgets
    budget = ROOT / "scripts" / "check-module-budgets.mjs"
    bt = budget.read_text(encoding="utf-8")
    # tighten dictation + commands and register new files
    updates = [
        (
            '  { path: "coordinator/dictation.rs", maxLines: 6500 },\n'
            '  { path: "coordinator/dictation_preview.rs", maxLines: 1500 },',
            '  { path: "coordinator/dictation.rs", maxLines: 3500 },\n'
            '  { path: "coordinator/dictation_preview.rs", maxLines: 1500 },\n'
            '  { path: "coordinator/dictation_device_ai.rs", maxLines: 1200 },\n'
            '  { path: "coordinator/dictation_wake_polish.rs", maxLines: 1200 },\n'
            '  { path: "coordinator/dictation_session.rs", maxLines: 1200 },\n'
            '  { path: "coordinator/dictation_embedded_submit.rs", maxLines: 1000 },\n'
            '  { path: "coordinator/dictation_embedded_stream.rs", maxLines: 2000 },',
        ),
        (
            '  { path: "commands/mod.rs", maxLines: 9000 },',
            '  { path: "commands/mod.rs", maxLines: 3500 },\n'
            '  { path: "commands/style_pack_commands.rs", maxLines: 800 },\n'
            '  { path: "commands/local_asr_commands.rs", maxLines: 800 },\n'
            '  { path: "commands/diagnostics_export.rs", maxLines: 1200 },\n'
            '  { path: "commands/marketplace.rs", maxLines: 1200 },',
        ),
    ]
    for old, new in updates:
        if old not in bt:
            raise SystemExit(f"budget missing block: {old[:80]}")
        bt = bt.replace(old, new)
    budget.write_text(bt, encoding="utf-8", newline="\n")
    print("budget updated")

    # expand dictation_tests concat
    dt = ROOT / "src-tauri" / "src" / "coordinator" / "dictation_tests.rs"
    dtt = dt.read_text(encoding="utf-8")
    extras = [
        "dictation_preview.rs",
        "dictation_device_ai.rs",
        "dictation_wake_polish.rs",
        "dictation_session.rs",
        "dictation_embedded_submit.rs",
        "dictation_embedded_stream.rs",
    ]
    # replace any concat or bare include for dictation sources
    new_concat = (
        "concat!("
        + ", ".join(f'include_str!("{name}")' for name in ["dictation.rs", *extras[0:]])
        # better multi-line with newlines between
    )
    # build pretty concat
    parts = ['include_str!("dictation.rs")'] + [f'include_str!("{x}")' for x in extras]
    # insert newlines in source string
    body = ',\n        "\\n",\n        '.join(parts)
    new_concat = f"concat!(\n        {body}\n    )"
    # replace existing concat for dictation
    dtt2, n = re.subn(
        r'concat!\(\s*include_str!\("dictation\.rs"\)[\s\S]*?\)',
        new_concat,
        dtt,
    )
    if n == 0:
        # bare includes only
        dtt2 = dtt.replace(
            'include_str!("dictation.rs")',
            new_concat,
        )
        n = dtt.count('include_str!("dictation.rs")')
    dt.write_text(dtt2, encoding="utf-8", newline="\n")
    print(f"dictation_tests concat updated n~{n}")

    # commands_tests if includes commands/mod
    ct = ROOT / "src-tauri" / "src" / "commands_tests.rs"
    if ct.exists():
        ctt = ct.read_text(encoding="utf-8")
        if 'include_str!("commands' in ctt or 'include_str!("mod.rs")' in ctt:
            print("commands_tests has include_str - check manually")
        # common pattern include_str for commands mod via path
        if 'include_str!("../commands/mod.rs")' in ctt or 'include_str!("mod.rs")' in ctt:
            print("commands_tests path includes present")

    print("phase3 extract done")


if __name__ == "__main__":
    main()
