"""Point source-structure tests at embedded_ble/{mod,windows_ble}.rs after the split."""
from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src-tauri" / "src"

WIN_MACRO_OLD = """    macro_rules! include_str {
        (\"embedded_ble.rs\") => {
            std::include_str!(\"embedded_ble.rs\").replace(\"\\r\\n\", \"\\n\")
        };
    }"""

WIN_MACRO_NEW = """    macro_rules! include_str {
        (\"embedded_ble.rs\") => {{
            concat!(
                std::include_str!(\"mod.rs\"),
                \"\\n\",
                std::include_str!(\"windows_ble.rs\")
            )
            .replace(\"\\r\\n\", \"\\n\")
        }};
    }"""

COMBINED_FROM_DIR = (
    'concat!(include_str!("mod.rs"), "\\n", include_str!("windows_ble.rs"))'
)
COMBINED_FROM_SRC = (
    'concat!(include_str!("embedded_ble/mod.rs"), "\\n", '
    'include_str!("embedded_ble/windows_ble.rs"))'
)


def main() -> None:
    win = SRC / "embedded_ble" / "windows_ble.rs"
    text = win.read_text(encoding="utf-8")
    if WIN_MACRO_OLD not in text:
        raise SystemExit("windows_ble include_str macro block not found")
    win.write_text(text.replace(WIN_MACRO_OLD, WIN_MACRO_NEW, 1), encoding="utf-8", newline="\n")
    print("windows_ble macro OK")

    mod = SRC / "embedded_ble" / "mod.rs"
    text = mod.read_text(encoding="utf-8")
    old = 'include_str!("embedded_ble.rs")'
    count = text.count(old)
    if count == 0:
        raise SystemExit("no include_str!(embedded_ble.rs) in mod.rs")
    mod.write_text(text.replace(old, COMBINED_FROM_DIR), encoding="utf-8", newline="\n")
    print(f"mod.rs tests: {count} replacements")

    coord = SRC / "coordinator_tests.rs"
    text = coord.read_text(encoding="utf-8")
    old = 'std::include_str!("embedded_ble.rs").replace("\\r\\n", "\\n")'
    new = f'{COMBINED_FROM_SRC}.replace("\\r\\n", "\\n")'
    if old not in text:
        old = 'include_str!("embedded_ble.rs").replace("\\r\\n", "\\n")'
        if old not in text:
            raise SystemExit("coordinator_tests pattern missing")
    coord.write_text(text.replace(old, new), encoding="utf-8", newline="\n")
    print("coordinator_tests OK")

    startup = SRC / "startup_evidence.rs"
    text = startup.read_text(encoding="utf-8")
    old = 'include_str!("embedded_ble.rs").replace("\\r\\n", "\\n")'
    new = f'{COMBINED_FROM_SRC}.replace("\\r\\n", "\\n")'
    if old not in text:
        raise SystemExit("startup_evidence pattern missing")
    startup.write_text(text.replace(old, new), encoding="utf-8", newline="\n")
    print("startup_evidence OK")


if __name__ == "__main__":
    main()
