"""Fix coordinator include! paths, budgets, and include_str macros after split."""
from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main() -> None:
    coord = ROOT / "src-tauri" / "src" / "coordinator.rs"
    text = coord.read_text(encoding="utf-8")
    text = text.replace(
        'include!("hotkey_device_runtime.rs");',
        'include!("coordinator/hotkey_device_runtime.rs");',
    )
    text = text.replace(
        'include!("embedded_ble_runtime.rs");',
        'include!("coordinator/embedded_ble_runtime.rs");',
    )
    coord.write_text(text, encoding="utf-8", newline="\n")
    print("includes:", [line for line in text.splitlines() if "include!" in line])

    budget = ROOT / "scripts" / "check-module-budgets.mjs"
    bt = budget.read_text(encoding="utf-8")
    old = '  { path: "coordinator.rs", maxLines: 9500 },'
    new = (
        '  { path: "coordinator.rs", maxLines: 4500 },\n'
        '  { path: "coordinator/hotkey_device_runtime.rs", maxLines: 2500 },\n'
        '  { path: "coordinator/embedded_ble_runtime.rs", maxLines: 4000 },'
    )
    if old not in bt:
        raise SystemExit(f"budget entry missing: {old!r}")
    budget.write_text(bt.replace(old, new), encoding="utf-8", newline="\n")
    print("budget updated")

    # coordinator_tests: expand include_str!("coordinator.rs") macro
    tests = ROOT / "src-tauri" / "src" / "coordinator_tests.rs"
    tt = tests.read_text(encoding="utf-8")
    old_macro = '''    ("coordinator.rs") => {
        std::include_str!("coordinator.rs").replace("\\r\\n", "\\n")
    };'''
    new_macro = '''    ("coordinator.rs") => {
        concat!(
            std::include_str!("coordinator.rs"),
            "\\n",
            std::include_str!("coordinator/hotkey_device_runtime.rs"),
            "\\n",
            std::include_str!("coordinator/embedded_ble_runtime.rs")
        )
        .replace("\\r\\n", "\\n")
    };'''
    if old_macro not in tt:
        raise SystemExit("coordinator_tests macro not found")
    tests.write_text(tt.replace(old_macro, new_macro), encoding="utf-8", newline="\n")
    print("coordinator_tests macro updated")

    # startup_evidence: expand coordinator include if present as bare include_str
    se = ROOT / "src-tauri" / "src" / "startup_evidence.rs"
    st = se.read_text(encoding="utf-8")
    old_se = 'let coordinator = include_str!("coordinator.rs");'
    new_se = '''let coordinator = concat!(
            include_str!("coordinator.rs"),
            "\\n",
            include_str!("coordinator/hotkey_device_runtime.rs"),
            "\\n",
            include_str!("coordinator/embedded_ble_runtime.rs")
        );'''
    if old_se in st:
        se.write_text(st.replace(old_se, new_se), encoding="utf-8", newline="\n")
        print("startup_evidence updated")
    else:
        print("startup_evidence already expanded or missing")


if __name__ == "__main__":
    main()
