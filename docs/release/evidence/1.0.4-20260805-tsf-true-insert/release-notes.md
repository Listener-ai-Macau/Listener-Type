# Listener Type 1.0.4 true-insert path (2026-08-05)

## Root cause
Default MSI **does not** register `ListenerTypeIme.dll`. ActivateProfile was doomed (0x80004005) and insert preferred clipboard → `PasteSent`.

## Fix
1. Skip ActivateProfile retries when TSF is not registered.
2. Prefer **IME-safe Unicode SendInput** (temporary en-US layout armor) → `Inserted`.
3. Clipboard paste remains reliability fallback.
4. Streaming path `switch_to_ascii` on Windows now also switches layout.

## Identity
| 项 | 值 |
|---|---|
| MSI SHA256 | 0755FF8CEFA40D280EBF77F9B811F5E8D21170F92CB4529DA3C6AD70DEEB8AEF |
| EXE SHA256 | 90E692AE72492558923F090345D384AC3F98F6283FA9A87222CBF642BD667DB8 |
| 载荷一致 | True |

## Hand-test
Look for logs:
- `TSF not registered (...); skip activate`
- `non-TSF IME-safe Unicode insert status=Inserted`
