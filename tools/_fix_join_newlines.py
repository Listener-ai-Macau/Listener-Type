"""Fix accidental real-newline .join("\n") breakage in JS scripts."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
for rel in [
    "scripts/check-performance-baselines.mjs",
    "scripts/check-embedded-ble-processing-led.mjs",
]:
    p = ROOT / rel
    if not p.exists():
        continue
    text = p.read_text(encoding="utf-8")
    fixed, n = re.subn(r"\]\.join\([\s\S]*?\);", r'].join("\\n");', text, count=1)
    p.write_text(fixed, encoding="utf-8", newline="\n")
    raw = p.read_bytes()
    i = raw.find(b"].join")
    print(rel, "replacements", n, "bytes", list(raw[i : i + 14]) if i >= 0 else None)
