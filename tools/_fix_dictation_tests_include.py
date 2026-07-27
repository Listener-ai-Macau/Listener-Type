from pathlib import Path

p = Path(__file__).resolve().parents[1] / "src-tauri" / "src" / "coordinator" / "dictation_tests.rs"
t = p.read_text(encoding="utf-8")
old = 'include_str!("dictation.rs")'
new = 'concat!(include_str!("dictation.rs"), "\\n", include_str!("dictation_preview.rs"))'
count = t.count(old)
if count == 0:
    raise SystemExit("no include_str dictation.rs found")
p.write_text(t.replace(old, new), encoding="utf-8", newline="\n")
print(f"replaced {count}")

# LED join byte fix
led = Path(__file__).resolve().parents[1] / "scripts" / "check-embedded-ble-processing-led.mjs"
raw = led.read_bytes()
if b'].join("' in raw:
    # find and ensure join is ].join("\n"); with escaped n
    text = led.read_text(encoding="utf-8")
    import re

    text2 = re.sub(r"\]\.join\(\s*\"[^\"]*\"\s*\);", '].join("\\n");', text, count=1)
    # if the join argument was a real newline, the pattern above may not match well
    text2 = re.sub(r"\]\.join\(\s*\"\s*\"\s*\);", '].join("\\n");', text2)
    led.write_text(text2, encoding="utf-8", newline="\n")
    print("led join normalized", list(led.read_bytes()[led.read_bytes().find(b"].join") : led.read_bytes().find(b"].join") + 15]))
