# Permissions

## macOS

Listener Type needs:

- Microphone: record speech.
- Accessibility: listen to the global hotkey and insert text into the active app.
- Input Monitoring may be requested by the platform for low-level hotkey handling.

After granting Accessibility, fully quit and reopen Listener Type. macOS applies the trust decision to a new process.

## Windows

Listener Type needs:

- Microphone privacy permission.
- Global hotkey hook availability.
- Optional TSF IME registration for the Windows insertion bridge.

Run `scripts/windows-preflight.ps1` and `scripts/windows-runtime-smoke.ps1` on a Windows runner when validating packaging.

## Data And Credentials

- App data: `Listener Type` under the platform data directory.
- Logs: `Listener Type/Logs/listener-type.log`.
- Credential service: `com.listener.type`.
