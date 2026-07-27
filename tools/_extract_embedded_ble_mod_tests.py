"""Extract #[cfg(test)] mod tests from embedded_ble/mod.rs into mod_tests.rs via #[path]."""
from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MOD = ROOT / "src-tauri" / "src" / "embedded_ble" / "mod.rs"
TESTS = ROOT / "src-tauri" / "src" / "embedded_ble" / "mod_tests.rs"


def main() -> None:
    full = MOD.read_text(encoding="utf-8")
    lines = full.splitlines(keepends=True)

    start = None
    for i, line in enumerate(lines):
        if line.startswith("#[cfg(test)]") and i + 1 < len(lines) and lines[i + 1].startswith(
            "mod tests {"
        ):
            start = i
            break
    if start is None:
        raise SystemExit("tests module not found")

    spec = importlib.util.spec_from_file_location(
        "split", ROOT / "tools" / "_split_embedded_ble.py"
    )
    scanner = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(scanner)

    marker = "mod tests {"
    brace_at = full.find(marker, sum(len(l) for l in lines[:start]))
    if brace_at < 0:
        raise SystemExit("mod tests { not found")
    brace_at = brace_at + len(marker) - 1
    end = scanner.find_matching_brace(full, brace_at)

    body = full[brace_at + 1 : end]
    if body.startswith("\n"):
        body = body[1:]
    dedented = []
    for line in body.splitlines(keepends=True):
        if line.startswith("    "):
            dedented.append(line[4:])
        else:
            dedented.append(line)
    if dedented and not dedented[-1].endswith("\n"):
        dedented[-1] += "\n"

    header = [
        "//! Path-separated unit tests for `embedded_ble` (mod.rs).\n",
        '//! Loaded via `#[path = "mod_tests.rs"]` from `mod.rs`.\n',
        "\n",
        "use super::*;\n",
        "\n",
    ]
    tests_text = "".join(header + dedented)
    if tests_text.count("use super::*;") >= 2:
        parts = tests_text.split("use super::*;\n", 2)
        if len(parts) == 3:
            tests_text = parts[0] + "use super::*;\n" + parts[2]

    TESTS.write_text(tests_text, encoding="utf-8", newline="\n")

    before = full[: sum(len(l) for l in lines[:start])].rstrip() + "\n\n"
    stub = '#[cfg(test)]\n#[path = "mod_tests.rs"]\nmod tests;\n'
    after = full[end + 1 :].lstrip("\n")
    MOD.write_text(before + stub + (("\n" + after) if after else ""), encoding="utf-8", newline="\n")

    print(f"wrote {TESTS} ({sum(1 for _ in TESTS.open(encoding='utf-8'))} lines)")
    print(f"rewrote {MOD} ({sum(1 for _ in MOD.open(encoding='utf-8'))} lines)")


if __name__ == "__main__":
    main()
