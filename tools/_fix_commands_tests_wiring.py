"""Remove inline commands tests and fix commands_tests.rs include paths."""
from __future__ import annotations

import re
from pathlib import Path

SRC = Path(__file__).resolve().parents[1] / "src-tauri" / "src"
MOD = SRC / "commands" / "mod.rs"
TESTS = SRC / "commands_tests.rs"


def main() -> None:
    lines = MOD.read_text(encoding="utf-8").splitlines(keepends=True)
    start = None
    wrap = None
    for i, l in enumerate(lines):
        if (
            start is None
            and l.startswith("#[cfg(test)]")
            and i + 1 < len(lines)
            and lines[i + 1].startswith("mod tests {")
        ):
            start = i
        if "// ── device domain thin Tauri wrappers" in l:
            wrap = i
    if start is not None and wrap is not None and start < wrap:
        print(f"removing inline tests {start + 1}-{wrap}")
        lines = lines[:start] + lines[wrap:]
    else:
        print("markers", start, wrap)

    text = "".join(lines)
    text = re.sub(
        r"\n#\[cfg\(test\)\]\n#\[path = \"[^\"]+\"\]\nmod tests;\n?",
        "\n",
        text,
    )
    path_mod = (
        "\n#[cfg(test)]\n"
        '#[path = "../commands_tests.rs"]\n'
        "mod tests;\n"
    )
    text = text.rstrip() + "\n" + path_mod
    MOD.write_text(text, encoding="utf-8")
    print("mod.rs lines", len(text.splitlines()))
    print("mod tests occurrences", len(re.findall(r"\bmod tests\b", text)))

    if not TESTS.exists():
        print("no commands_tests.rs")
        return

    t = TESTS.read_text(encoding="utf-8")

    def manifest_include(rel_from_src_tauri: str) -> str:
        # rel like src/commands/mod.rs
        return (
            'include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/'
            + rel_from_src_tauri
            + '"))'
        )

    # Replace any include_str!("...") used for source-structure tests.
    mapping = {
        'include_str!("commands/mod.rs")': manifest_include("src/commands/mod.rs"),
        'include_str!("commands.rs")': manifest_include("src/commands/mod.rs"),
        'include_str!("commands/device.rs")': manifest_include(
            "src/commands/device/settings.rs"
        ),
        'include_str!("../commands/device.rs")': manifest_include(
            "src/commands/device/settings.rs"
        ),
        'include_str!("coordinator.rs")': manifest_include("src/coordinator.rs"),
        'include_str!("../coordinator.rs")': manifest_include("src/coordinator.rs"),
        'include_str!("lib.rs")': manifest_include("src/lib.rs"),
        'include_str!("../lib.rs")': manifest_include("src/lib.rs"),
    }
    for old, new in mapping.items():
        if old in t:
            t = t.replace(old, new)
            print("replaced", old)

    TESTS.write_text(t, encoding="utf-8")
    print("include_str count", t.count("include_str!"))


if __name__ == "__main__":
    main()
