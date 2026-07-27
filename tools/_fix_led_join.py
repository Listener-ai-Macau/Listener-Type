from pathlib import Path
import re

p = Path(__file__).resolve().parents[1] / "scripts" / "check-embedded-ble-processing-led.mjs"
text = p.read_text(encoding="utf-8")
# Replace any broken ].join(...) with a proper escaped newline join.
fixed = re.sub(r"\]\.join\([\s\S]*?\);", r'].join("\\n");', text, count=1)
p.write_text(fixed, encoding="utf-8", newline="\n")
raw = p.read_bytes()
i = raw.find(b"].join")
print("bytes", list(raw[i : i + 14]))
assert raw[i : i + 14] == b'].join("\\n");', raw[i : i + 20]
print("ok")
