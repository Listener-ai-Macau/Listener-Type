"""
Split monolithic src-tauri/src/embedded_ble.rs into:

  embedded_ble/mod.rs           — shared types, helpers, public wrappers, tests
  embedded_ble/windows_ble.rs   — Windows-only BLE implementation (was nested mod)

Does not change product behavior: only module layout for AI maintainability.
Handles Rust raw strings (r#"..."#) so PowerShell/script braces do not end the module early.
"""
from __future__ import annotations

import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src-tauri" / "src"
SRC_FILE = SRC / "embedded_ble.rs"
OUT_DIR = SRC / "embedded_ble"
WIN_FILE = OUT_DIR / "windows_ble.rs"
MOD_FILE = OUT_DIR / "mod.rs"
BACKUP = SRC / "embedded_ble.rs.bak-split"


def find_matching_brace(text: str, open_index: int) -> int:
    """Return index of the '}' matching text[open_index] == '{', scanning Rust syntax."""
    assert text[open_index] == "{"
    i = open_index + 1
    depth = 1
    n = len(text)

    while i < n:
        ch = text[i]

        # Line comment
        if ch == "/" and i + 1 < n and text[i + 1] == "/":
            i += 2
            while i < n and text[i] != "\n":
                i += 1
            continue

        # Block comment
        if ch == "/" and i + 1 < n and text[i + 1] == "*":
            i += 2
            while i + 1 < n and not (text[i] == "*" and text[i + 1] == "/"):
                i += 1
            i = min(i + 2, n)
            continue

        # Raw string: r#"..."# / r##"..."## / br#"..."#
        if ch == "r" or (ch == "b" and i + 1 < n and text[i + 1] == "r"):
            j = i + 1 if ch == "r" else i + 2
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                # find closing "###...
                j += 1
                close = '"' + ("#" * hashes)
                while j < n:
                    if text.startswith(close, j):
                        i = j + len(close)
                        break
                    j += 1
                else:
                    raise RuntimeError("unterminated raw string")
                continue

        # Byte string b"..."
        if ch == "b" and i + 1 < n and text[i + 1] == '"':
            i += 2
            while i < n:
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                i += 1
            continue

        # Normal string
        if ch == '"':
            i += 1
            while i < n:
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                i += 1
            continue

        # Char literal 'x' / '\n' / '{' — skip carefully
        if ch == "'":
            # lifetime 'a or 'static — not a char if next is ident-ish and no close soon
            if i + 1 < n and (text[i + 1].isalpha() or text[i + 1] == "_"):
                j = i + 2
                while j < n and (text[j].isalnum() or text[j] == "_"):
                    j += 1
                # if no closing quote immediately after lifetime-ish, treat as lifetime
                if j < n and text[j] != "'":
                    i = j
                    continue
            i += 1
            if i < n and text[i] == "\\":
                i += 2
            elif i < n:
                i += 1
            if i < n and text[i] == "'":
                i += 1
            continue

        if ch == "{":
            depth += 1
            i += 1
            continue
        if ch == "}":
            depth -= 1
            if depth == 0:
                return i
            i += 1
            continue

        i += 1

    raise RuntimeError("unmatched '{'")


def dedent_block(lines: list[str], spaces: int = 4) -> list[str]:
    prefix = " " * spaces
    out: list[str] = []
    for line in lines:
        if line.startswith(prefix):
            out.append(line[spaces:])
        elif line.strip() == "":
            out.append("\n" if line.endswith("\n") else (line if line else "\n"))
        else:
            out.append(line)
    return out


def main() -> None:
    if not SRC_FILE.exists():
        raise SystemExit(f"missing {SRC_FILE}; restore from backup first")

    text = SRC_FILE.read_text(encoding="utf-8")
    # Normalize to \n for scanning; preserve content
    text_n = text.replace("\r\n", "\n").replace("\r", "\n")

    marker = "mod windows_ble {"
    start = text_n.find(marker)
    if start < 0:
        raise SystemExit("mod windows_ble { not found")
    brace_at = start + len(marker) - 1
    assert text_n[brace_at] == "{"

    end = find_matching_brace(text_n, brace_at)

    # Line numbers for logging
    line_of = lambda idx: text_n.count("\n", 0, idx) + 1
    print(f"windows_ble brace: open line {line_of(brace_at)}, close line {line_of(end)}")

    # cfg attribute line start
    before_mod = text_n.rfind("\n", 0, start)
    cfg_start = before_mod + 1 if before_mod >= 0 else 0
    # include #[cfg(...)] on previous line if present
    prev_nl = text_n.rfind("\n", 0, cfg_start - 1) if cfg_start > 0 else -1
    prev_line_start = prev_nl + 1
    prev_line = text_n[prev_line_start:cfg_start]
    if 'target_os = "windows"' in prev_line:
        region_start = prev_line_start
    else:
        region_start = cfg_start

    # Body between { and }
    body_text = text_n[brace_at + 1 : end]
    # drop leading newline for cleaner file
    if body_text.startswith("\n"):
        body_text = body_text[1:]
    body_lines = [ln + "\n" for ln in body_text.split("\n")]
    # last split gives empty if trailing newline — fix
    if body_lines and body_lines[-1] == "\n" and not body_text.endswith("\n"):
        body_lines[-1] = ""
    # Better: splitlines keepends
    body_lines = body_text.splitlines(keepends=True)
    if body_lines and not body_lines[-1].endswith("\n"):
        body_lines[-1] += "\n"
    body_lines = dedent_block(body_lines, 4)

    header = [
        "//! Windows-only BLE implementation for Listener embedded audio.\n",
        "//!\n",
        "//! Extracted from the former nested `mod windows_ble` in `embedded_ble.rs`\n",
        "//! for AI maintainability. Parent module owns shared types and public wrappers.\n",
        "\n",
    ]

    before = text_n[:region_start]
    after = text_n[end + 1 :]
    # strip trailing blanks from before
    before = before.rstrip("\n") + "\n\n"
    # strip leading blanks from after
    after = after.lstrip("\n")
    if after and not after.startswith("\n"):
        after = "\n" + after if not after.startswith("#") else after
    # ensure after starts cleanly
    if after and not after.startswith("\n"):
        after = "\n" + after

    decl = '#[cfg(target_os = "windows")]\nmod windows_ble;\n\n'
    mod_text = before + decl + after.lstrip("\n")
    # if after already has leading content, keep single blank after decl
    if not mod_text.endswith("\n"):
        mod_text += "\n"

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    WIN_FILE.write_text("".join(header + body_lines), encoding="utf-8", newline="\n")
    MOD_FILE.write_text(mod_text, encoding="utf-8", newline="\n")

    if not BACKUP.exists():
        shutil.copy2(SRC_FILE, BACKUP)
        print(f"backup -> {BACKUP}")
    else:
        print(f"backup already exists: {BACKUP}")

    SRC_FILE.unlink()
    print(f"removed {SRC_FILE}")

    for p in (MOD_FILE, WIN_FILE):
        n = sum(1 for _ in p.open(encoding="utf-8"))
        print(f"  {p.relative_to(ROOT)}: {n} lines")


if __name__ == "__main__":
    main()
