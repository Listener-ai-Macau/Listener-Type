"""
Migrate tools/_recovered_modules/device/* into src-tauri/src/commands/device/
and rebuild commands/mod.rs by removing only matching top-level symbols.
"""
from __future__ import annotations

import re
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src-tauri" / "src"
REC_DEV = ROOT / "tools" / "_recovered_modules" / "device"
CMD_RS = SRC / "commands.rs"
OUT = SRC / "commands"
DEVICE_OUT = OUT / "device"

ITEM_RE = re.compile(
    r"^(pub(\([^)]*\))?\s+)?(async\s+)?(fn|struct|enum|type|const|static)\s+([A-Za-z0-9_]+)",
    re.M,
)


def strip_strings_and_comments(line: str) -> str:
    """Roughly blank out comments/strings so brace counting ignores them."""
    # line comments
    if "//" in line:
        in_str = False
        out = []
        i = 0
        while i < len(line):
            ch = line[i]
            if ch == '"' and (i == 0 or line[i - 1] != "\\"):
                in_str = not in_str
                out.append(" ")
                i += 1
                continue
            if not in_str and line[i : i + 2] == "//":
                break
            out.append(ch if not in_str else " ")
            i += 1
        line = "".join(out)
    # naive string wipe
    out = []
    in_str = False
    i = 0
    while i < len(line):
        ch = line[i]
        if ch == '"' and (i == 0 or line[i - 1] != "\\"):
            in_str = not in_str
            out.append(" ")
            i += 1
            continue
        out.append(" " if in_str else ch)
        i += 1
    return "".join(out)


def brace_delta(line: str) -> int:
    s = strip_strings_and_comments(line)
    return s.count("{") - s.count("}")


def expand_start_with_attrs(lines: list[str], i: int) -> int:
    start = i
    j = i - 1
    while j >= 0:
        t = lines[j].strip()
        if (
            lines[j].startswith("#[")
            or t.startswith("///")
            or t.startswith("//!")
            or t.startswith("//")
            or t == ""
        ):
            if t == "":
                if j > 0 and (
                    lines[j - 1].startswith("#[")
                    or lines[j - 1].strip().startswith("///")
                ):
                    start = j
                    j -= 1
                    continue
                break
            start = j
            j -= 1
            continue
        break
    return start


def span_braced_item(lines: list[str], i: int) -> int:
    """Return end index of braced block starting at line i, or line ending with ;."""
    n = len(lines)
    depth = 0
    seen = False
    k = i
    while k < n:
        depth += brace_delta(lines[k])
        if "{" in strip_strings_and_comments(lines[k]):
            seen = True
        if seen and depth <= 0:
            return k
        if not seen and lines[k].rstrip().endswith(";"):
            return k
        k += 1
    raise SystemExit(f"unclosed braced item at line {i + 1}")


def find_items(lines: list[str]) -> list[dict]:
    """Return top-level items with start/end 0-based inclusive indices."""
    items = []
    i = 0
    n = len(lines)
    while i < n:
        m = ITEM_RE.match(lines[i])
        if not m:
            i += 1
            continue
        kind, name = m.group(4), m.group(5)
        start = expand_start_with_attrs(lines, i)

        if kind in {"fn", "struct", "enum"}:
            end = span_braced_item(lines, i)
            items.append(
                {
                    "start": start,
                    "end": end,
                    "kind": kind,
                    "name": name,
                    "sig_line": i,
                }
            )
            i = end + 1
        else:
            # type/const/static single line (maybe multi-line with ;)
            k = i
            while k < n and not lines[k].rstrip().endswith(";"):
                k += 1
            items.append(
                {
                    "start": start,
                    "end": k,
                    "kind": kind,
                    "name": name,
                    "sig_line": i,
                }
            )
            i = k + 1
    return items


IMPL_RE = re.compile(
    r"^impl(?:\s*<[^>]*>)?\s+(?:(?P<trait>[\w:]+(?:\s*<[^>]*>)?)\s+for\s+)?"
    r"(?P<type>[\w:]+)(?:\s*<[^>]*>)?\s*\{"
)


def find_impl_spans_for_types(lines: list[str], type_names: set[str]) -> list[tuple[int, int]]:
    """Find top-level impl blocks whose Self type is in type_names."""
    spans: list[tuple[int, int]] = []
    i = 0
    n = len(lines)
    while i < n:
        raw = lines[i].strip()
        # allow attributes above impl
        if not raw.startswith("impl") and not (
            raw.startswith("pub") and "impl" in raw
        ):
            # also match bare impl after attrs only at item start
            if not re.match(r"^impl\b", lines[i]):
                i += 1
                continue
        m = re.match(
            r"^(?:pub(\([^)]*\))?\s+)?impl(?:\s*<[^>{]*>)?\s+"
            r"(?:([\w:]+)(?:\s*<[^>]*>)?\s+for\s+)?"
            r"([\w:]+)",
            lines[i],
        )
        if not m:
            i += 1
            continue
        # group: optional trait, type
        # pattern: impl [Trait for] Type
        full = lines[i]
        # extract type name more carefully
        type_name = None
        if re.search(r"\bfor\s+([A-Za-z_][A-Za-z0-9_]*)", full):
            type_name = re.search(r"\bfor\s+([A-Za-z_][A-Za-z0-9_]*)", full).group(1)
        else:
            # impl Type or impl Type<...>
            mm = re.match(
                r"^(?:pub(\([^)]*\))?\s+)?impl(?:\s*<[^>{]*>)?\s+([A-Za-z_][A-Za-z0-9_]*)",
                full,
            )
            if mm:
                type_name = mm.group(2)
        start = expand_start_with_attrs(lines, i)
        end = span_braced_item(lines, i)
        if type_name and type_name in type_names:
            spans.append((start, end))
        i = end + 1
    return spans


def is_tauri_command(lines: list[str], item: dict) -> bool:
    for idx in range(item["start"], item["sig_line"] + 1):
        if "tauri::command" in lines[idx]:
            return True
    return False


def extract_fn_signature(lines: list[str], item: dict) -> str:
    """Return signature text from first attr/fn line through ')' before '{'."""
    chunk = "".join(lines[item["start"] : item["end"] + 1])
    # cut at first '{' of body
    brace = chunk.find("{")
    if brace < 0:
        return chunk.strip()
    return chunk[:brace].rstrip()


def parse_fn_params(sig: str) -> list[str]:
    """Extract argument names for forwarding (best-effort)."""
    m = re.search(r"fn\s+[A-Za-z0-9_]+\s*(?:<[^>]*>)?\s*\((.*)\)\s*(?:->|\s*$)", sig, re.S)
    if not m:
        return []
    params = m.group(1).strip()
    if not params:
        return []
    # split on commas at depth 0
    parts = []
    depth = 0
    cur = []
    for ch in params:
        if ch in "(<[":
            depth += 1
        elif ch in ")>]":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append("".join(cur).strip())
            cur = []
            continue
        cur.append(ch)
    if cur:
        parts.append("".join(cur).strip())
    names = []
    for p in parts:
        p = p.strip()
        if not p:
            continue
        # patterns: name: Type, mut name: Type, coord: CoordinatorState<'_>
        p = re.sub(r"^mut\s+", "", p)
        name = p.split(":")[0].strip()
        if name and name != "self":
            names.append(name)
    return names


def make_wrapper(sig: str, name: str, is_async: bool) -> str:
    params = parse_fn_params(sig)
    args = ", ".join(params)
    call = f"device::{name}({args})"
    if is_async:
        call = f"{call}.await"
    # ensure signature ends cleanly
    sig = sig.rstrip()
    if not sig.endswith(")"):
        # might end with -> Type spanning lines; ok
        pass
    return f"{sig} {{\n    {call}\n}}\n"


def device_symbol_names() -> set[str]:
    names: set[str] = set()
    for p in REC_DEV.glob("*.rs"):
        text = p.read_text(encoding="utf-8")
        for m in ITEM_RE.finditer(text):
            names.add(m.group(5))
    return names


def write_device_tree() -> None:
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir()
    DEVICE_OUT.mkdir()
    for name in ("ble.rs", "settings.rs", "firmware.rs"):
        text = (REC_DEV / name).read_text(encoding="utf-8")
        # normalize imports: parent commands symbols via super::super
        text = text.replace("use super::*;\n", "use super::super::*;\n")
        # settings already has super::super and super::ble
        if name == "settings.rs":
            # ensure no use super::* left that would pull device reexports
            text = text.replace("use super::*;\n", "")
        (DEVICE_OUT / name).write_text(text, encoding="utf-8")

    (DEVICE_OUT / "mod.rs").write_text(
        """//! Device / BLE / firmware domain implementations.
//! Thin Tauri wrappers remain in `commands/mod.rs`.

mod ble;
mod settings;
mod firmware;

pub use ble::*;
pub use settings::*;
pub use firmware::*;
""",
        encoding="utf-8",
    )


def main() -> None:
    if not CMD_RS.exists():
        raise SystemExit(f"missing {CMD_RS}")
    if not REC_DEV.exists():
        raise SystemExit(f"missing {REC_DEV}")

    raw = CMD_RS.read_text(encoding="utf-8")
    lines = raw.splitlines(keepends=True)
    # normalize to keepends consistent
    lines = [l if l.endswith("\n") else l + "\n" for l in raw.splitlines()]

    names = device_symbol_names()
    print(f"device symbols: {len(names)}")
    items = find_items(lines)
    print(f"top-level items in commands.rs: {len(items)}")

    # Types moved to device need their impl blocks removed from commands too.
    type_names = {
        it["name"] for it in items if it["kind"] in {"struct", "enum"} and it["name"] in names
    }
    # Also include type names defined only in recovered device files.
    for p in REC_DEV.glob("*.rs"):
        t = p.read_text(encoding="utf-8")
        for m in re.finditer(
            r"(?m)^(?:pub(?:\([^)]*\))?\s+)?(struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)",
            t,
        ):
            type_names.add(m.group(2))

    remove_idxs: set[int] = set()
    wrappers: list[str] = []
    removed = []
    for it in items:
        if it["name"] not in names:
            continue
        for idx in range(it["start"], it["end"] + 1):
            remove_idxs.add(idx)
        removed.append(it["name"])
        if it["kind"] == "fn" and is_tauri_command(lines, it):
            sig = extract_fn_signature(lines, it)
            is_async = "async fn" in sig
            wrappers.append(make_wrapper(sig, it["name"], is_async))

    impl_spans = find_impl_spans_for_types(lines, type_names)
    for a, b in impl_spans:
        for idx in range(a, b + 1):
            remove_idxs.add(idx)

    print(
        f"removing {len(removed)} items + {len(impl_spans)} impls, "
        f"{len(remove_idxs)} lines, wrappers={len(wrappers)}"
    )

    remain = [ln for i, ln in enumerate(lines) if i not in remove_idxs]
    text = "".join(remain)

    inject = """
pub mod device;
pub use device::{
    DeviceSettingsSnapshot, DeviceSettingsUpdateRequest, EmbeddedBleRecoveryAction,
    EmbeddedBleRepairResult, EmbeddedBleRuntimeStatus, FirmwareOtaBleTransferResult,
    FirmwareOtaPackagePayload, FirmwareOtaPreflightSnapshot, WiredFirmwareArtifactInfo,
    WiredFirmwareFlashResult, WiredFirmwarePackagePayload, WiredFirmwareProgressPayload,
    WiredFirmwareSerialPort,
};
pub(crate) use device::*;

"""
    if "pub mod device;" not in text:
        if re.search(r"type CoordinatorState[^;]+;", text):
            text = re.sub(
                r"(type CoordinatorState[^;]+;\n)",
                r"\1" + inject,
                text,
                count=1,
            )
        else:
            text = inject + text

    # Qualify free calls still in commands body
    for name in (
        "device_firmware_settings_changed",
        "validate_device_firmware_preferences",
        "sync_device_firmware_preferences",
    ):
        text = re.sub(rf"(?<![:\w]){name}\(", f"device::{name}(", text)
    text = text.replace("device::device::", "device::")

    if wrappers:
        text = text.rstrip() + "\n\n// ── device domain thin Tauri wrappers ──\n\n"
        text += "\n".join(wrappers)
        if not text.endswith("\n"):
            text += "\n"

    # tests path if recovered tests exist
    tests_src = ROOT / "tools" / "_recovered_modules" / "commands_tests.rs"
    if tests_src.exists():
        shutil.copy(tests_src, SRC / "commands_tests.rs")
        if "commands_tests.rs" not in text[-600:]:
            text += '\n#[cfg(test)]\n#[path = "../commands_tests.rs"]\nmod tests;\n'

    write_device_tree()
    (OUT / "mod.rs").write_text(text, encoding="utf-8")
    CMD_RS.unlink()

    print("commands/mod.rs lines", len(text.splitlines()))
    for p in sorted(OUT.rglob("*.rs")):
        print(f"  {len(p.read_text(encoding='utf-8').splitlines()):5} {p.relative_to(SRC)}")
    # sanity: critical commands still present
    for must in ("get_settings", "set_settings", "list_history", "get_credentials"):
        ok = re.search(rf"(?m)^(?:pub\s+)?(?:async\s+)?fn\s+{must}\b", text) is not None
        print(f"  keep {must}: {ok}")


if __name__ == "__main__":
    main()
