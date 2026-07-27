"""Extract #[cfg(test)] mod tests from windows_ble.rs into windows_ble/tests.rs via #[path]."""
from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WIN = ROOT / "src-tauri" / "src" / "embedded_ble" / "windows_ble.rs"
# Prefer file-style sibling first; if we use directory later, adjust.
# Keep windows_ble as a single file module: use #[path = "windows_ble_tests.rs"] mod tests;
TESTS = ROOT / "src-tauri" / "src" / "embedded_ble" / "windows_ble_tests.rs"


def main() -> None:
    text = WIN.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)

    start = None
    for i, line in enumerate(lines):
        if line.startswith("#[cfg(test)]") and i + 1 < len(lines) and lines[i + 1].startswith("mod tests {"):
            start = i
            break
    if start is None:
        raise SystemExit("tests module not found")

    # find matching brace of mod tests {
    def strip(line: str) -> str:
        # reuse simple: ignore strings roughly by wiping quotes content
        out = []
        in_s = False
        i = 0
        while i < len(line):
            ch = line[i]
            if ch == '"' and (i == 0 or line[i - 1] != "\\"):
                in_s = not in_s
                out.append(" ")
                i += 1
                continue
            if not in_s and line[i : i + 2] == "//":
                break
            out.append(" " if in_s else ch)
            i += 1
        return "".join(out)

    # Prefer the raw-string-aware scanner from split script for the tests block only if needed.
    # tests block is at end and should not contain huge raw scripts with unbalanced braces at depth 0...
    # but may have raw strings. Use char scanner briefly.
    full = "".join(lines)
    # locate mod tests { in full from start line offset
    offset = sum(len(l) for l in lines[: start + 1])  # up to and including #[cfg]
    # actually start points at #[cfg(test)]\nmod tests {
    marker = "mod tests {"
    brace_at = full.find(marker, sum(len(l) for l in lines[:start]))
    if brace_at < 0:
        raise SystemExit("mod tests { not found in full text")
    brace_at = brace_at + len(marker) - 1

    # import scanner
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "split", ROOT / "tools" / "_split_embedded_ble.py"
    )
    mod = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(mod)
    end = mod.find_matching_brace(full, brace_at)

    # body
    body = full[brace_at + 1 : end]
    if body.startswith("\n"):
        body = body[1:]
    body_lines = body.splitlines(keepends=True)
    # dedent 4
    dedented = []
    for line in body_lines:
        if line.startswith("    "):
            dedented.append(line[4:])
        else:
            dedented.append(line)
    if dedented and not dedented[-1].endswith("\n"):
        dedented[-1] += "\n"

    header = [
        "//! Path-separated unit tests for `embedded_ble::windows_ble`.\n",
        "//! Loaded via `#[path = \"windows_ble_tests.rs\"]` from `windows_ble.rs`.\n",
        "\n",
        "use super::*;\n",
        "\n",
    ]

    # Fix include_str macro: paths are relative to this file (same dir as windows_ble.rs)
    # Keep the macro that loads both module sources.
    tests_text = "".join(header + dedented)
    # The extracted body already has `use super::*;` and macro - remove duplicate use if present at top of body
    # Body started with:
    #     use super::*;
    # after dedent: use super::*;
    # We already added use super::*; - remove first body one if duplicate
    if tests_text.count("use super::*;") >= 2:
        # remove only the first after header - body one
        parts = tests_text.split("use super::*;\n", 2)
        if len(parts) == 3:
            tests_text = parts[0] + "use super::*;\n" + parts[2]

    TESTS.write_text(tests_text, encoding="utf-8", newline="\n")

    # Replace tests module in windows_ble with path mod
    before = full[: sum(len(l) for l in lines[:start])]
    # drop trailing blanks
    before = before.rstrip() + "\n\n"
    stub = (
        '#[cfg(test)]\n'
        '#[path = "windows_ble_tests.rs"]\n'
        "mod tests;\n"
    )
    # anything after end brace?
    after = full[end + 1 :].lstrip("\n")
    WIN.write_text(before + stub + (("\n" + after) if after else ""), encoding="utf-8", newline="\n")

    print(f"wrote {TESTS} ({sum(1 for _ in TESTS.open(encoding='utf-8'))} lines)")
    print(f"rewrote {WIN} ({sum(1 for _ in WIN.open(encoding='utf-8'))} lines)")


if __name__ == "__main__":
    main()
