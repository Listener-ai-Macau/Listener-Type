import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const appRoot = join(scriptsDir, "..");
const repoRoot = appRoot;
const scriptPath = join(scriptsDir, "windows-package-msvc.ps1");
const launcherPath = join(scriptsDir, "windows-package-msvc.cmd");
const ciWorkflowPath = join(repoRoot, ".github", "workflows", "release-tauri.yml");
const imeBuildPath = join(scriptsDir, "windows-ime-build.ps1");
const imeInstallSmokePath = join(scriptsDir, "windows-ime-install-smoke.ps1");
const imeRegisterPath = join(scriptsDir, "windows-ime-register.ps1");
const imeUnregisterPath = join(scriptsDir, "windows-ime-unregister.ps1");
const imeSolutionPath = join(appRoot, "windows-ime", "ListenerTypeIme.sln");
const imeProjectPath = join(appRoot, "windows-ime", "ListenerTypeIme.vcxproj");
const imeEditSessionPath = join(appRoot, "windows-ime", "src", "edit_session.cpp");
const imeTextServicePath = join(appRoot, "windows-ime", "src", "text_service.cpp");
const packageJsonPath = join(appRoot, "package.json");
const persistencePath = join(appRoot, "src-tauri", "src", "persistence.rs");
const tauriConfigPath = join(appRoot, "src-tauri", "tauri.conf.json");
const viteConfigPath = join(appRoot, "vite.config.ts");
const distIndexPath = join(appRoot, "dist", "index.html");
const nsisHookPath = join(appRoot, "src-tauri", "nsis", "listener-type-ime-hooks.nsh");
const nsisCleanupHookPath = join(appRoot, "src-tauri", "nsis", "listener-type-ime-cleanup-hooks.nsh");
const regressionGatePath = join(scriptsDir, "run-v1.0.2-regression.ps1");
const releaseCheckPath = join(scriptsDir, "release-check.mjs");
const releaseRootArtifactsPath = join(scriptsDir, "check-release-root-artifacts.mjs");
const wixFragmentPath = join(appRoot, "src-tauri", "wix", "listener-type-ime.wxs");
const wixCleanupFragmentPath = join(appRoot, "src-tauri", "wix", "listener-type-ime-cleanup.wxs");

const script = readFileSync(scriptPath, "utf8");
const launcher = readFileSync(launcherPath, "utf8");
const ciWorkflow = readFileSync(ciWorkflowPath, "utf8");
const imeBuild = readFileSync(imeBuildPath, "utf8");
const imeInstallSmoke = readFileSync(imeInstallSmokePath, "utf8");
const imeRegister = readFileSync(imeRegisterPath, "utf8");
const imeUnregister = readFileSync(imeUnregisterPath, "utf8");
const imeSolution = readFileSync(imeSolutionPath, "utf8");
const imeProject = readFileSync(imeProjectPath, "utf8");
const imeEditSession = readFileSync(imeEditSessionPath, "utf8");
const imeTextService = readFileSync(imeTextServicePath, "utf8");
const packageJsonText = readFileSync(packageJsonPath, "utf8");
const persistence = readFileSync(persistencePath, "utf8");
const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
const viteConfig = readFileSync(viteConfigPath, "utf8");
const distIndex = readFileSync(distIndexPath, "utf8");
const nsisHook = readFileSync(nsisHookPath, "utf8");
const nsisCleanupHook = readFileSync(nsisCleanupHookPath, "utf8");
const regressionGate = readFileSync(regressionGatePath, "utf8");
const releaseCheck = readFileSync(releaseCheckPath, "utf8");
const releaseRootArtifacts = readFileSync(releaseRootArtifactsPath, "utf8");
const wixFragment = readFileSync(wixFragmentPath, "utf8");
const wixCleanupFragment = readFileSync(wixCleanupFragmentPath, "utf8");

const requiredFragments = [
  "Install-RustMsvcToolchain",
  "https://win.rustup.rs/x86_64",
  "stable-x86_64-pc-windows-msvc",
  "Find-VsDevCmd",
  "VsDevCmd.bat",
  "npm.cmd ci",
  "tauri build -- --target x86_64-pc-windows-msvc --bundles msi",
  "Repair-TauriMsiBundle",
  "Enable-SameVersionMsiUpgrade",
  "Find-BuiltMsiPath",
  "Install-LatestMsi",
  "Start-InstalledListenerType",
  "msiexec.exe",
  "/qn",
  "/norestart",
  "/L*v",
  "Installed Listener Type exe matches MSI payload",
  "candle.exe",
  "light.exe",
  "-sice:ICE03",
  "-sice:ICE40",
  "-sice:ICE57",
  "-sice:ICE61",
  "main.wixobj",
  "listener-type-ime-cleanup.wixobj",
  "locale.wxl",
  "AllowSameVersionUpgrades=\"yes\"",
  "DowngradeErrorMessage",
  "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
  "WebView2Loader.dll",
  "Compress-Archive",
  "Get-FileHash -Algorithm SHA256",
];

for (const fragment of requiredFragments) {
  assert.match(script, new RegExp(fragment.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), `missing ${fragment}`);
}

assert.match(script, /\[switch\]\$SkipRustInstall/, "script should support opting out of Rust installation");
assert.match(script, /\[switch\]\$SkipNpmCi/, "script should support reusing existing node_modules");
assert.match(script, /\[switch\]\$IncludePortable/, "portable zip generation should require an explicit opt-in switch");
assert.match(script, /\[switch\]\$CleanArtifacts/, "script should support cleaning the output directory");
assert.match(script, /\[int\]\$CargoBuildJobs = 0/, "script should leave Cargo parallelism at the default unless explicitly limited");
assert.match(script, /\[switch\]\$ReuseExistingExe/, "script should support fast MSI relink for docs/scripts-only commits");
assert.match(script, /\[switch\]\$SkipDesktopShortcut/, "script should support opting out of local desktop shortcut refresh");
assert.match(script, /\[switch\]\$UseSccache/, "script should support opt-in sccache acceleration");
assert.ok(
  script.indexOf('if ($UseSccache.IsPresent -and -not (Test-Command "sccache"))')
    < script.indexOf('Write-Host "[info] Install validation requested; stopping installed Listener Type before the long MSI build"'),
  'missing sccache must fail before install validation stops the user-facing app',
);
assert.match(script, /\[switch\]\$IncrementalReleaseBuild/, "script should support opt-in local incremental release builds");
assert.match(script, /\[switch\]\$InstallMsi/, "script should support installing the freshly built MSI");
assert.match(script, /\[switch\]\$LaunchInstalledApp/, "script should support launching the installed Program Files app after MSI update");
assert.match(script, /\[int\]\$CommandTimeoutSeconds = 3600/, "packaging should bound long child commands by default");
assert.match(script, /\[System\.IO\.Path\]::IsPathRooted\(\$ArtifactsRoot\)/, "packaging should normalize relative ArtifactsRoot before passing MSI paths to msiexec");
assert.match(script, /\$ArtifactsRoot = \[System\.IO\.Path\]::GetFullPath\(\$ArtifactsRoot\)/, "packaging should use absolute artifact paths for msiexec and shortcut validation");
assert.match(script, /Invoke-CmdWithHeartbeat[\s\S]*TimeoutSeconds[\s\S]*taskkill\.exe \/PID \$process\.Id \/T \/F/, "heartbeat runner should terminate hung child process trees");
assert.match(script, /Tauri Windows MSI build[\s\S]*-TimeoutSeconds \$CommandTimeoutSeconds/, "Tauri MSI build should use the bounded heartbeat runner");
assert.match(script, /Invoke-CmdWithHeartbeat -Command "npm\.cmd ci"[\s\S]*-TimeoutSeconds \$CommandTimeoutSeconds/, "npm ci should use the bounded heartbeat runner");
assert.match(script, /msiexec\.exe \/i[\s\S]*Invoke-CmdWithHeartbeat -Command \$installCommand[\s\S]*-TimeoutSeconds \$CommandTimeoutSeconds/, "MSI install should use the bounded heartbeat runner");
assert.match(regressionGate, /-IncrementalReleaseBuild/, "v1.0.2 regression packaging should use incremental release builds to avoid repeated long relinks");
assert.match(regressionGate, /C:\\Program Files\\Listener Type\\listener-type\.exe/, "v1.0.2 regression should default to the installed Program Files app");
assert.match(regressionGate, /-InstallMsi/, "v1.0.2 regression package gate must install the freshly built MSI");
assert.match(regressionGate, /-LaunchInstalledApp/, "v1.0.2 regression package gate must start the installed app after MSI update");
assert.doesNotMatch(regressionGate, /\$ReleaseExePath = Join-Path \$tauriRoot "target\\x86_64-pc-windows-msvc\\release\\listener-type\.exe"/, "v1.0.2 regression must not default to launching the repo release exe");
assert.match(releaseCheck, /check:preproduction-operator-notes/, "release:check must reject untriaged operator notes before publishing");
assert.match(releaseCheck, /check:msi-upgrade-contract/, "release:check must enforce same-version MSI replacement before publishing");
assert.match(releaseCheck, /check:preproduction-bench-contract/, "release:check must keep the preproduction bench/human workflow contract in the default release gate");
assert.match(releaseCheck, /check:ota-speed-log-contract/, "release:check must reject stale OTA speed logs that hide a newer failed transfer");
assert.match(packageJsonText, /"check:release-root-artifacts": "node scripts\/check-release-root-artifacts\.mjs"/, "package.json should expose the final Denzic-root package gate");
assert.match(packageJsonText, /"check:msi-upgrade-contract": "node scripts\/windows-package-msvc\.test\.mjs"/, "package.json should expose the same-version MSI replacement contract");
assert.match(releaseRootArtifacts, /ListenerType_\$\{version\}_x64_en-US\.msi/, "release root artifact gate should require the current Type MSI");
assert.match(releaseRootArtifacts, /ListenerFirmware_\$\{version\}_ota\.zip/, "release root artifact gate should require the current Firmware OTA zip");
assert.match(releaseRootArtifacts, /portable.*\.zip/i, "release root artifact gate should reject Type portable zips");
assert.match(releaseRootArtifacts, /--type-source[\s\S]*--firmware-source/, "release root artifact gate should support source-hash comparison during final staging");
assert.match(script, /Test-ReusableReleaseExe/, "script should guard the fast MSI relink path");
assert.match(script, /function Update-DesktopShortcut/, "packaging should protect the local desktop shortcut after each rebuild");
assert.match(script, /WScript\.Shell/, "desktop shortcut refresh should use the Windows shortcut COM API");
assert.match(script, /Listener Type\.lnk/, "desktop shortcut refresh should target the stable Listener Type shortcut name");
assert.match(script, /C:\\Program Files\\Listener Type\\listener-type\.exe/, "desktop shortcut refresh must target the installed MSI application");
assert.match(script, /installed Listener Type exe does not match the freshly built MSI payload/, "desktop shortcut refresh must refuse stale installed apps");
assert.match(script, /Desktop shortcut refreshed/, "desktop shortcut refresh must rewrite the user-facing shortcut after every MSI update");
assert.match(script, /\$refreshedShortcut = \$shell\.CreateShortcut\(\$shortcutPath\)/, "desktop shortcut refresh must reread the saved link before reporting success");
assert.match(script, /Desktop shortcut verification failed after refresh/, "desktop shortcut refresh must fail instead of claiming success when the saved link is missing or stale");
assert.match(script, /CommonDesktopDirectory/, "desktop shortcut refresh must reuse the MSI-owned common desktop location when present");
assert.match(script, /Removed duplicate user desktop shortcut/, "desktop shortcut refresh must clean up its duplicate user-desktop link after an MSI update");
assert.match(script, /\$isScriptManagedDuplicate/, "desktop shortcut cleanup must only remove the link created by this packaging script");
assert.match(script, /-LaunchInstalledApp requires -InstallMsi/, "launching the app must require a freshly installed MSI in validation");
assert.match(script, /Install validation requested; stopping installed Listener Type before the long MSI build[\s\S]*Stop-InstalledListenerType[\s\S]*Stop-RunningReleaseApp[\s\S]*Invoke-MsvcBuild/, "install validation must stop the Program Files app before the long MSI build so users do not keep interacting with a stale or stuck exe");
assert.match(script, /Stop-InstalledListenerType[\s\S]*msiexec\.exe[\s\S]*Assert-InstalledPayloadMatchesRelease/, "MSI update must stop the installed app, install with msiexec, and verify the Program Files payload hash");
assert.match(script, /--local-wake-helper[\s\S]*Leaving Listener Type local wake helper to exit with its parent/, "MSI shutdown must not relaunch --quit for the helper after the GUI parent exits");
assert.match(script, /Assert-InstalledPayloadMatchesRelease[\s\S]*TimeoutSeconds[\s\S]*Start-Sleep -Milliseconds 250/, "MSI validation must wait for the Program Files payload hash to settle after install");
assert.match(script, /lastHashError[\s\S]*catch[\s\S]*Exception\.Message/, "MSI validation must retry through transient Program Files file locks");
assert.match(script, /Start-Process -FilePath \$installedExePath[\s\S]*Started installed Listener Type app/, "installed-app validation must launch the Program Files exe");
assert.match(script, /AddSeconds\(5\)[\s\S]*startup stability gate/, "installed-app validation must keep the Program Files process alive through the delayed startup-crash window");
assert.match(script, /Get-WinEvent[\s\S]*Windows\\\.Devices\\\.Bluetooth\\\.dll[\s\S]*0xc0000005/, "installed-app validation must reject the retained WinRT Bluetooth access-violation signature");
assert.match(releaseCheck, /check:dirty-inventory/, "release:check must classify dirty work before the clean-worktree gate reports release blockage");
assert.match(releaseCheck, /check:guard-commits/, "release:check must keep regression guards linked to recoverable repository commits");
assert.match(releaseCheck, /check:repo-hygiene/, "release:check must require a clean worktree before publishing or final review");
assert.doesNotMatch(script, /Description = "Listener Type latest local release build"/, "desktop shortcut must not advertise or target the repo build output");
assert.match(script, /Cargo build jobs left at Cargo default parallelism/, "script should advertise default Cargo parallelism");
assert.doesNotMatch(script, /set `"CARGO_BUILD_JOBS=1`"/, "default packaging must not force serial Cargo builds");
assert.doesNotMatch(script, /WixTools314/, "MSVC packaging must not hard-code a single Tauri WiX tools version");
assert.doesNotMatch(ciWorkflow, /WixTools314/, "CI MSI repair must not hard-code a single Tauri WiX tools version");
assert.match(script, /-Filter "WixTools\*"/, "MSVC packaging should discover Tauri WiX tools by WixTools* glob");
assert.match(ciWorkflow, /WixTools\*\\light\.exe/, "CI MSI repair should discover Tauri WiX tools by WixTools* glob");

assert.equal(tauriConfig.bundle.windows.nsis.installMode, "perMachine", "Windows installer remains a machine-wide product install");
assert.equal(tauriConfig.bundle.windows.nsis.installerHooks, "nsis/listener-type-ime-cleanup-hooks.nsh", "default NSIS may only clean up legacy TSF IME registration");
assert.deepEqual(tauriConfig.bundle.windows.wix.fragmentPaths, ["wix/listener-type-ime-cleanup.wxs"], "default MSI may only include legacy TSF cleanup actions");
assert.deepEqual(tauriConfig.bundle.windows.wix.componentRefs, ["LegacyListenerTypeImeRegistryCleanupComponent"], "default MSI may only include the legacy TSF registry cleanup component");
assert.match(viteConfig, /base:\s*["']\.\/["']/, "Tauri production builds must use relative asset URLs");
assert.doesNotMatch(distIndex, /\b(?:src|href)="\/assets\//, "built dist/index.html must not reference absolute /assets URLs");
assert.match(distIndex, /\b(?:src|href)="\.\/assets\//, "built dist/index.html should reference relative ./assets URLs");
assert.doesNotMatch(script, /Invoke-ListenerTypeImeBuild/, "default packaging must not build IME DLLs");
assert.doesNotMatch(script, /LISTENER_TYPE_IME_DLL_X64/, "default packaging must not require an x64 IME DLL");
assert.doesNotMatch(script, /LISTENER_TYPE_IME_DLL_X86/, "default packaging must not require an x86 IME DLL");
assert.doesNotMatch(script, /listener-type-ime\.wixobj/, "default MSI repair must not link the IME WiX object");
assert.match(script, /Listener Type_\$\(Get-PackageVersion\)_x64_en-US\.msi/, "packaging should accept Tauri's product-name MSI output");
assert.match(script, /Copy-Item -LiteralPath \$msiPath -Destination \(Join-Path \$ArtifactsRoot \$msiName\)/, "packaging should copy the built MSI to the stable ListenerType artifact name");
assert.match(script, /Remove-Item -LiteralPath \$portableRoot -Recurse -Force -ErrorAction SilentlyContinue/, "default packaging should remove stale portable folders");
assert.match(script, /Remove-Item -LiteralPath \$zipPath -Force -ErrorAction SilentlyContinue/, "default packaging should remove stale portable zips");
assert.match(script, /if \(\$IncludePortable\) \{[\s\S]*Compress-Archive/, "portable zip generation should stay behind IncludePortable");
assert.match(script, /Invoke-MsvcBuild[\s\S]*Repair-TauriMsiBundle[\s\S]*Copy-WindowsArtifacts[\s\S]*Install-LatestMsi[\s\S]*Update-DesktopShortcut[\s\S]*Start-InstalledListenerType/, "packaging should relink the Tauri MSI, install it when requested, refresh the desktop shortcut, and start the installed app");
assert.match(script, /AllowSameVersionUpgrades="yes"/, "MSI packaging should allow same-version 1.0.0 replacement builds to major-upgrade installed copies");
assert.match(script, /DowngradeErrorMessage=/, "MSI packaging should keep an explicit downgrade block after replacing AllowDowngrades");
assert.match(persistence, /const PREFERENCES_FILE: &str = "preferences\.json"/, "user settings should live in a stable preferences.json file");
assert.match(persistence, /target_os = "windows"[\s\S]*std::env::var\("APPDATA"\)[\s\S]*join\(app_profile_dir_name\(\)\)/, "Windows preferences must live under %APPDATA%\\Listener Type, independent of the installed exe");
assert.doesNotMatch(script, /preferences\.json/i, "MSI packaging and install validation must not rewrite the user preferences file");
assert.doesNotMatch(script, /APPDATA[\s\S]{0,240}Remove-Item|Remove-Item[\s\S]{0,240}APPDATA/i, "MSI packaging and install validation must not delete or clean the user AppData settings directory");
assert.doesNotMatch(ciWorkflow, /windows-ime-install-smoke\.ps1/, "release CI must not expect default installers to register a TSF IME");
assert.match(ciWorkflow, /node scripts\/windows-package-msvc\.test\.mjs/, "release CI should run the static packaging guard");

assert.match(imeBuild, /\[string\]\$OutputDirectory/, "standalone IME build should support a package-specific output directory");
assert.match(imeBuild, /\[string\]\$IntermediateDirectory/, "standalone IME build should support a package-specific intermediate directory");
assert.match(imeBuild, /\[ValidateSet\("x64", "Win32"\)\]/, "standalone IME build should support x64 and Win32 platforms");
assert.match(imeBuild, /\/p:Platform=\$Platform/, "standalone IME build should pass Platform to MSBuild");
assert.match(imeBuild, /\$defaultOutputDirectory = Join-Path \$appRoot "windows-ime\\\$defaultPlatformFolder\\\$Configuration"/, "standalone IME build should force stable default OutDir per platform");
assert.match(imeBuild, /\/p:OutDir=/, "standalone IME build should pass OutDir to MSBuild");
assert.match(imeBuild, /\/p:IntDir=/, "standalone IME build should pass IntDir to MSBuild");
assert.match(imeRegister, /windows-ime-build\.ps1/, "manual IME register should build before registering");
assert.doesNotMatch(imeRegister, /if \(-not \(Test-Path \$dll\)\)/, "manual IME register must rebuild stale DLLs, not only missing DLLs");
assert.match(imeRegister, /windows-ime-register/, "manual IME register should use a side-by-side staging output to avoid locked registered DLLs");
assert.match(imeRegister, /Get-Date/, "manual IME register should create a fresh staging output for each registration run");
assert.match(imeRegister, /\$PID/, "manual IME register should include the process id in the staging output to avoid path reuse");
assert.match(imeRegister, /-OutputDirectory/, "manual IME register should pass a staging output directory to the build script");
assert.match(imeRegister, /-IntermediateDirectory/, "manual IME register should pass a staging intermediate directory to the build script");
assert.match(imeRegister, /active-registration\.json/, "manual IME register should persist the staged DLL paths it registered");
assert.match(imeUnregister, /active-registration\.json/, "manual IME unregister should read the registered staged DLL manifest");
assert.match(imeUnregister, /windows-ime-register/, "manual IME unregister should target the same staging root used by register");
assert.match(imeUnregister, /ConvertFrom-Json/, "manual IME unregister should parse persisted registered DLL paths");
assert.doesNotMatch(imeUnregister, /windows-ime\\\$folder\\\$Configuration\\ListenerTypeIme\.dll/, "manual IME unregister must not only derive legacy build-output DLL paths");

assert.match(imeSolution, /Release\|Win32/, "IME solution should include a Win32 Release configuration");
assert.match(imeProject, /Release\|Win32/, "IME project should include a Win32 Release configuration");
assert.match(imeTextService, /TF_E_SYNCHRONOUS/, "IME should detect hosts like Word that reject synchronous edit sessions");
assert.match(imeTextService, /TF_ES_ASYNC \| TF_ES_READWRITE/, "IME should retry Word-hosted commits with an async edit session");
assert.match(imeTextService, /WaitForSingleObject/, "IME pipe submit should wait for async edit-session completion");
assert.match(imeEditSession, /SetEvent/, "IME edit session should signal async completion back to the pipe submitter");
assert.match(imeEditSession, /Collapse\(edit_cookie, TF_ANCHOR_END\)/, "IME should collapse the committed range to its end after insertion");
assert.match(imeEditSession, /SetSelection\(edit_cookie, 1, &selection\)/, "IME should move the caret to the end of inserted text");
assert.match(imeEditSession, /TF_AE_END/, "IME should make the end of the committed text the active selection end");

assert.match(wixFragment, /Component Id="ListenerTypeImeDllX64Component"/, "optional IME WiX fragment should still define the x64 TSF DLL component");
assert.match(wixFragment, /Component Id="ListenerTypeImeDllX86Component"/, "optional IME WiX fragment should still define the x86 TSF DLL component");
assert.match(wixFragment, /regsvr32\.exe/, "optional IME WiX fragment should still register and unregister the TSF DLL when deliberately wired in");
assert.match(nsisHook, /NSIS_HOOK_POSTINSTALL/, "optional IME NSIS hook should still support TSF DLL registration when deliberately wired in");
assert.match(nsisHook, /NSIS_HOOK_PREUNINSTALL/, "optional IME NSIS hook should still support TSF DLL unregistration when deliberately wired in");
assert.match(wixCleanupFragment, /UnregisterLegacyListenerTypeImeX64OnInstall/, "default MSI should unregister a previously installed x64 TSF IME");
assert.match(wixCleanupFragment, /UnregisterLegacyListenerTypeImeX86OnInstall/, "default MSI should unregister a previously installed x86 TSF IME");
assert.match(wixCleanupFragment, /LegacyListenerTypeImeRegistryCleanupComponent/, "default MSI should include a registry cleanup component");
assert.match(wixCleanupFragment, /RemoveRegistryKey Root="HKLM" Key="Software\\Microsoft\\CTF\\TIP\\\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767\}"/, "default MSI should remove the legacy TSF TIP key");
assert.doesNotMatch(wixCleanupFragment, /Component Id="ListenerTypeImeDll/, "default MSI cleanup fragment must not install IME DLL components");
assert.doesNotMatch(wixCleanupFragment, /RegisterListenerTypeImeX64/, "default MSI cleanup fragment must not register the TSF IME");
assert.match(nsisCleanupHook, /LISTENER_TYPE_LEGACY_IME_UNREGISTER_X64/, "default NSIS hook should unregister a previously installed x64 TSF IME");
assert.match(nsisCleanupHook, /LISTENER_TYPE_LEGACY_IME_REMOVE_REGISTRY/, "default NSIS hook should remove stale TSF registry keys");
assert.match(nsisCleanupHook, /LISTENER_TYPE_LEGACY_IME_REMOVE_FILES/, "default NSIS hook should remove stale bundled IME DLLs");
assert.doesNotMatch(nsisCleanupHook, /LISTENER_TYPE_IME_REGISTER_X64/, "default NSIS cleanup hook must not register the TSF IME");

assert.match(imeInstallSmoke, /\[ValidateSet\("nsis", "msi"\)\]/, "manual IME install smoke should support both Windows installers");
assert.match(imeInstallSmoke, /Join-ProcessArguments/, "manual IME install smoke should quote process arguments before Start-Process");
assert.match(imeInstallSmoke, /Start-Process -FilePath \$FilePath -ArgumentList \$commandLine/, "manual IME install smoke should pass a single quoted command line to Start-Process");
assert.match(imeInstallSmoke, /ListenerTypeImeSubmit/, "manual IME install smoke should preserve TSF backend context");
assert.match(imeInstallSmoke, /Software\\Classes\\CLSID\\\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767\}\\InprocServer32/, "manual IME install smoke should check x64 COM registration");
assert.match(imeInstallSmoke, /LanguageProfile\\0x00000804\\\{19F96D43-A5EB-46C9-8A73-9FCA5A0630C8\}/, "manual IME install smoke should check the TSF language profile");

assert.match(launcher, /pwsh\.exe/, "launcher should call pwsh.exe");
assert.doesNotMatch(launcher, new RegExp("-ExecutionPolicy" + " Bypass"), "launcher should not carry Windows PowerShell execution-policy shims");
assert.match(launcher, /windows-package-msvc\.ps1/, "launcher should invoke the packaging script");
assert.match(launcher, /%SUPPLIED_ARGS%/, "launcher should forward user arguments");
