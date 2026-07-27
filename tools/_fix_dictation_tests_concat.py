from pathlib import Path
import re

p = Path(__file__).resolve().parents[1] / "src-tauri" / "src" / "coordinator" / "dictation_tests.rs"
t = p.read_text(encoding="utf-8")
parts = [
    "dictation.rs",
    "dictation_preview.rs",
    "dictation_device_ai.rs",
    "dictation_wake_polish.rs",
    "dictation_session.rs",
    "dictation_embedded_submit.rs",
    "dictation_embedded_stream.rs",
]
nl = "\\n"
items = []
for i, name in enumerate(parts):
    items.append(f'include_str!("{name}")')
    if i < len(parts) - 1:
        items.append(f'"{nl}"')
# join with comma-newline-indent
good = "concat!(\n        " + ",\n        ".join(items) + "\n    )"

pat = re.compile(
    r'concat!\(\s*include_str!\("dictation\.rs"\)[\s\S]*?include_str!\("dictation_embedded_stream\.rs"\)\s*\)+'
)
t2, n = pat.subn(good, t)
if n == 0:
    # also try broken form with real newlines / double commas
    pat2 = re.compile(
        r'concat!\([\s\S]*?include_str!\("dictation_embedded_stream\.rs"\)\s*\)+'
    )
    t2, n = pat2.subn(good, t)
if n == 0:
    raise SystemExit("no concat blocks found")
# clean accidental ",,"
t2 = t2.replace('",,', '",')
p.write_text(t2, encoding="utf-8", newline="\n")
print(f"replaced {n}")
print(good)
