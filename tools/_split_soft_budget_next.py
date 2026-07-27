"""Extract next soft-budget include! chunks from dictation + windows_ble."""
from __future__ import annotations

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def extract_ranges(
    file_path: Path,
    ranges: list[tuple[str, int, int, list[str]]],
    include_prefix: str = "",
) -> None:
    """ranges: (filename, start_1based, end_1based_inclusive, header_lines). Extract high-to-low."""
    lines = file_path.read_text(encoding="utf-8").splitlines(keepends=True)
    for name, s, e, header in ranges:
        print(f"{file_path.name}: {name} L{s}-{e}")
        print("  start:", lines[s - 1][:90].rstrip())
        print("  end:", lines[e - 1][:90].rstrip())
        assert "fn " in lines[s - 1] or lines[s - 1].startswith("impl ") or lines[s - 1].startswith("pub")
        assert lines[e - 1].strip() == "}" or lines[e - 1].rstrip().endswith("}")

    for name, s, e, header in ranges:
        start, end = s - 1, e
        block = lines[start:end]
        out = file_path.parent / name
        out.write_text("".join(header + block), encoding="utf-8", newline="\n")
        print(f"  wrote {out.relative_to(ROOT)} ({sum(1 for _ in out.open(encoding='utf-8'))} lines)")
        before = lines[:start]
        after = lines[end:]
        while before and before[-1].strip() == "":
            before.pop()
        while after and after and after[0].strip() == "":
            after.pop(0)
        include_path = f"{include_prefix}{name}" if include_prefix else name
        lines = before + ["\n", f'include!("{include_path}");\n', "\n"] + after

    file_path.write_text("".join(lines), encoding="utf-8", newline="\n")
    print(
        f"  rewrote {file_path.relative_to(ROOT)} ({sum(1 for _ in file_path.open(encoding='utf-8'))} lines)"
    )
    print("  includes:", [ln.strip() for ln in lines if "include!" in ln])


def patch_budget() -> None:
    path = ROOT / "scripts" / "check-module-budgets.mjs"
    text = path.read_text(encoding="utf-8")
    replacements = [
        (
            '  { path: "coordinator/dictation.rs", maxLines: 7500 },',
            '  { path: "coordinator/dictation.rs", maxLines: 6500 },\n'
            '  { path: "coordinator/dictation_preview.rs", maxLines: 1500 },',
        ),
        (
            '  { path: "embedded_ble/windows_ble/mod.rs", maxLines: 8500 },',
            '  { path: "embedded_ble/windows_ble/mod.rs", maxLines: 6000 },',
        ),
        (
            '  { path: "embedded_ble/windows_ble/notify_open.rs", maxLines: 1500 },',
            '  { path: "embedded_ble/windows_ble/notify_open.rs", maxLines: 1500 },\n'
            '  { path: "embedded_ble/windows_ble/ota_open.rs", maxLines: 2500 },\n'
            '  { path: "embedded_ble/windows_ble/gatt_open.rs", maxLines: 1200 },\n'
            '  { path: "embedded_ble/windows_ble/unpair.rs", maxLines: 900 },',
        ),
    ]
    for old, new in replacements:
        if old not in text:
            raise SystemExit(f"budget missing: {old}")
        text = text.replace(old, new)
    path.write_text(text, encoding="utf-8", newline="\n")
    print("budget updated")


def extend_concat_lists() -> None:
    """Append new windows_ble include files to known concat sites."""
    win_extra = [
        "windows_ble/unpair.rs",
        "windows_ble/ota_open.rs",
        "windows_ble/gatt_open.rs",
    ]
    emb_extra = [f"embedded_ble/{p}" for p in win_extra]

    def add_after(text: str, anchor: str, extras: list[str], use_std: bool) -> str:
        if extras[0] in text:
            return text
        if use_std:
            token = f'std::include_str!("{anchor}")'
            insert = "".join(
                f',\n            "\\n",\n            std::include_str!("{p}")' for p in extras
            )
        else:
            token = f'include_str!("{anchor}")'
            insert = "".join(f',\n        "\\n",\n        include_str!("{p}")' for p in extras)
            # also handle 12-space indent variants used in some files
            if token not in text:
                token = f'include_str!("{anchor}")'
                insert = "".join(
                    f',\n            "\\n",\n            include_str!("{p}")' for p in extras
                )
        if token not in text:
            raise SystemExit(f"anchor not found: {token}")
        # only first occurrence that is not already followed by unpair
        pos = 0
        while True:
            i = text.find(token, pos)
            if i < 0:
                raise SystemExit(f"anchor not found live: {token}")
            after = text[i + len(token) : i + len(token) + 60]
            if "unpair.rs" in after:
                pos = i + 1
                continue
            return text[: i + len(token)] + insert + text[i + len(token) :]

    # windows_ble_tests
    p = ROOT / "src-tauri" / "src" / "embedded_ble" / "windows_ble_tests.rs"
    t = p.read_text(encoding="utf-8")
    t = add_after(t, "windows_ble/notify_open.rs", win_extra, use_std=True)
    p.write_text(t, encoding="utf-8", newline="\n")
    print("windows_ble_tests concat extended")

    # coordinator_tests embedded_ble macro
    p = ROOT / "src-tauri" / "src" / "coordinator_tests.rs"
    t = p.read_text(encoding="utf-8")
    t = add_after(t, "embedded_ble/windows_ble/notify_open.rs", emb_extra, use_std=True)
    p.write_text(t, encoding="utf-8", newline="\n")
    print("coordinator_tests emb concat extended")

    # startup_evidence
    p = ROOT / "src-tauri" / "src" / "startup_evidence.rs"
    t = p.read_text(encoding="utf-8")
    t = add_after(t, "embedded_ble/windows_ble/notify_open.rs", emb_extra, use_std=False)
    p.write_text(t, encoding="utf-8", newline="\n")
    print("startup_evidence concat extended")

    # mod_tests: replace every notify_open close of concat with extras
    p = ROOT / "src-tauri" / "src" / "embedded_ble" / "mod_tests.rs"
    t = p.read_text(encoding="utf-8")
    anchor = 'include_str!("windows_ble/notify_open.rs")'
    insert = "".join(f',\n        "\\n",\n        include_str!("{x}")' for x in win_extra)
    if "windows_ble/unpair.rs" not in t:
        t = t.replace(anchor, anchor + insert)
        p.write_text(t, encoding="utf-8", newline="\n")
        print("mod_tests concats extended", t.count("unpair.rs"))
    else:
        print("mod_tests already has unpair")

    # coordinator tests for dictation - if any include_str dictation
    # scripts: rebuild win file lists
    win_files = [
        "mod.rs",
        "ota_transfer.rs",
        "pairing.rs",
        "pnp_cache.rs",
        "recording_control.rs",
        "capture_events.rs",
        "notify_open.rs",
        "unpair.rs",
        "ota_open.rs",
        "gatt_open.rs",
    ]

    led = ROOT / "scripts" / "check-embedded-ble-processing-led.mjs"
    lt = led.read_text(encoding="utf-8")
    lines = ["const source = ["]
    for name in win_files:
        lines.append(
            f'  readFileSync(join(embeddedBleDir, "windows_ble", "{name}"), "utf8"),'
        )
    lines.append('  readFileSync(join(embeddedBleDir, "mod.rs"), "utf8"),')
    lines.append('].join("\\n");')
    new_src = "\n".join(lines)
    lt2, n = re.subn(r"const source = \[[\s\S]*?\]\.join\([\s\S]*?\);", new_src, lt, count=1)
    if n != 1:
        raise SystemExit(f"led replace n={n}")
    # ensure join uses escaped \n not real newline
    lt2 = lt2.replace('].join("\n");', '].join("\\n");')
    # fix if python wrote real newline between quotes
    lt2 = re.sub(r'\]\.join\("\s*"\);', '].join("\\n");', lt2)
    led.write_text(lt2, encoding="utf-8", newline="\n")
    print("led script rebuilt")

    perf = ROOT / "scripts" / "check-performance-baselines.mjs"
    pt = perf.read_text(encoding="utf-8")
    lines = [
        "const embeddedBle = [",
        '  readFileSync(join(repoRoot, "src-tauri", "src", "embedded_ble", "mod.rs"), "utf8"),',
    ]
    for name in win_files:
        lines.append(
            f'  readFileSync(join(repoRoot, "src-tauri", "src", "embedded_ble", "windows_ble", "{name}"), "utf8"),'
        )
    lines.append('].join("\\n");')
    new_emb = "\n".join(lines)
    pt2, n = re.subn(r"const embeddedBle = \[[\s\S]*?\]\.join\([\s\S]*?\);", new_emb, pt, count=1)
    if n != 1:
        raise SystemExit(f"perf replace n={n}")
    pt2 = re.sub(r'\]\.join\("\s*"\);', '].join("\\n");', pt2)
    perf.write_text(pt2, encoding="utf-8", newline="\n")
    print("perf script rebuilt")


def main() -> None:
    # --- dictation ---
    extract_ranges(
        ROOT / "src-tauri" / "src" / "coordinator" / "dictation.rs",
        [
            (
                "dictation_preview.rs",
                947,
                2006,
                [
                    "// Embedded audio partial preview / wake-phrase filtering helpers.\n",
                    "// Included into `coordinator::dictation` via `include!`.\n",
                    "\n",
                ],
            ),
        ],
    )

    # --- windows_ble (high-to-low) ---
    extract_ranges(
        ROOT / "src-tauri" / "src" / "embedded_ble" / "windows_ble" / "mod.rs",
        [
            (
                "gatt_open.rs",
                5955,
                6412,
                [
                    "// GATT characteristic open / session-ready helpers.\n",
                    "// Included into `windows_ble` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "ota_open.rs",
                3830,
                5933,
                [
                    "// Listener OTA v1 target open / scan / characteristic helpers.\n",
                    "// Included into `windows_ble` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "unpair.rs",
                514,
                1152 + (1967 - 1967),  # placeholder fixed below
                [
                    "// Unpair / BTHPORT cleanup entrypoints used by Type recovery.\n",
                    "// Included into `windows_ble` via `include!`.\n",
                    "\n",
                ],
            ),
        ],
    )

    patch_budget()
    extend_concat_lists()
    print("done")


if __name__ == "__main__":
    # Fix unpair end precisely: finalize_known_address_unpair_result ends ~1152 function start, need end line
    # Recompute with brace matcher before main extract for unpair
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "split", ROOT / "tools" / "_split_embedded_ble.py"
    )
    split = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(split)
    win = ROOT / "src-tauri" / "src" / "embedded_ble" / "windows_ble" / "mod.rs"
    text = win.read_text(encoding="utf-8")
    m = re.search(r"(?m)^fn finalize_known_address_unpair_result\b", text)
    brace = text.find("{", m.start())
    end = split.find_matching_brace(text, brace)
    unpair_end = text.count("\n", 0, end) + 1
    print("unpair_end", unpair_end)

    # patch main's unpair range by inlining a fixed main
    # --- dictation ---
    extract_ranges(
        ROOT / "src-tauri" / "src" / "coordinator" / "dictation.rs",
        [
            (
                "dictation_preview.rs",
                947,
                2006,
                [
                    "// Embedded audio partial preview / wake-phrase filtering helpers.\n",
                    "// Included into `coordinator::dictation` via `include!`.\n",
                    "\n",
                ],
            ),
        ],
    )

    extract_ranges(
        win,
        [
            (
                "gatt_open.rs",
                5955,
                6412,
                [
                    "// GATT characteristic open / session-ready helpers.\n",
                    "// Included into `windows_ble` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "ota_open.rs",
                3830,
                5933,
                [
                    "// Listener OTA v1 target open / scan / characteristic helpers.\n",
                    "// Included into `windows_ble` via `include!`.\n",
                    "\n",
                ],
            ),
            (
                "unpair.rs",
                514,
                unpair_end,
                [
                    "// Unpair / BTHPORT cleanup entrypoints used by Type recovery.\n",
                    "// Included into `windows_ble` via `include!`.\n",
                    "\n",
                ],
            ),
        ],
    )

    patch_budget()
    extend_concat_lists()

    # Fix LED join if real newline sneaked in
    led = ROOT / "scripts" / "check-embedded-ble-processing-led.mjs"
    raw = led.read_bytes()
    if b'].join("\n");' in raw or b"].join(\n" in raw:
        # replace join with broken newline
        text = led.read_text(encoding="utf-8")
        text = re.sub(r"\]\.join\(\s*\"\s*\"\s*\);", '].join("\\n");', text)
        led.write_text(text, encoding="utf-8", newline="\n")
        print("fixed led join bytes")
    print("ALL DONE")
