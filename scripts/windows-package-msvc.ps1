param(
  [string]$ArtifactsRoot = "",
  [ValidateRange(0, 64)]
  [int]$CargoBuildJobs = 0,
  [switch]$SkipRustInstall,
  [switch]$SkipNpmCi,
  [switch]$IncludePortable,
  [switch]$CleanArtifacts,
  [switch]$ReuseExistingExe,
  [switch]$SkipDesktopShortcut,
  [switch]$UseSccache,
  [switch]$IncrementalReleaseBuild,
  [switch]$InstallMsi,
  [switch]$LaunchInstalledApp,
  [ValidateRange(60, 14400)]
  [int]$CommandTimeoutSeconds = 3600
)

$ErrorActionPreference = "Stop"

if ($env:NODE_TLS_REJECT_UNAUTHORIZED -eq "0") {
  Remove-Item Env:NODE_TLS_REJECT_UNAUTHORIZED -ErrorAction SilentlyContinue
}

$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$releaseRoot = Join-Path $appRoot "src-tauri\target\x86_64-pc-windows-msvc\release"
if ([string]::IsNullOrWhiteSpace($ArtifactsRoot)) {
  $ArtifactsRoot = Join-Path $appRoot ".artifacts\windows-msvc"
}
if (-not [System.IO.Path]::IsPathRooted($ArtifactsRoot)) {
  $ArtifactsRoot = Join-Path $appRoot $ArtifactsRoot
}
$ArtifactsRoot = [System.IO.Path]::GetFullPath($ArtifactsRoot)

function Add-PathEntry($PathEntry) {
  if ([string]::IsNullOrWhiteSpace($PathEntry) -or -not (Test-Path $PathEntry)) {
    return
  }
  $entries = $env:PATH -split ";"
  if ($entries -notcontains $PathEntry) {
    $env:PATH = "$PathEntry;$env:PATH"
  }
}

function Test-Command($Name) {
  return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Install-RustMsvcToolchain {
  $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  Add-PathEntry $cargoBin

  $hasRustup = Test-Command "rustup"
  $hasCargo = Test-Command "cargo"
  $hasRustc = Test-Command "rustc"
  $hasToolchain = $false
  if ($hasRustup) {
    $toolchains = & cmd.exe /d /c "rustup toolchain list 2>nul"
    $hasToolchain = $LASTEXITCODE -eq 0 -and $toolchains -match "stable-x86_64-pc-windows-msvc"
  }

  if ($hasRustup -and $hasCargo -and $hasRustc -and $hasToolchain) {
    Write-Host "[ok] Rust MSVC toolchain already installed"
    return
  }

  if ($SkipRustInstall) {
    throw "Rust MSVC toolchain is missing. Re-run without -SkipRustInstall to install it automatically."
  }

  Write-Host "[info] Installing Rust stable-x86_64-pc-windows-msvc"
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $rustupInit = Join-Path $env:TEMP "rustup-init-x86_64-pc-windows-msvc.exe"
  Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit
  & $rustupInit -y --default-toolchain stable-x86_64-pc-windows-msvc

  Add-PathEntry $cargoBin
  & rustup toolchain install stable-x86_64-pc-windows-msvc
  & rustup default stable-x86_64-pc-windows-msvc

  if (-not (Test-Command "cargo") -or -not (Test-Command "rustc")) {
    throw "Rust installation finished, but cargo/rustc is still not available in PATH."
  }
}

function Find-VsDevCmd {
  $candidates = @()

  $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  if (Test-Path $vswhere) {
    $installPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
    if (-not [string]::IsNullOrWhiteSpace($installPath)) {
      $candidates += (Join-Path $installPath "Common7\Tools\VsDevCmd.bat")
    }
  }

  $candidates += @(
    (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat"),
    (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Professional\Common7\Tools\VsDevCmd.bat"),
    (Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\Enterprise\Common7\Tools\VsDevCmd.bat"),
    (Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat")
  )

  foreach ($candidate in $candidates) {
    if (Test-Path $candidate) {
      return (Resolve-Path $candidate).Path
    }
  }

  throw "VsDevCmd.bat not found. Install Visual Studio 2022 Build Tools with the Desktop development with C++ workload."
}

function Find-WixTool($Name) {
  $tauriWixRoot = Join-Path $env:LOCALAPPDATA "tauri"
  if (Test-Path $tauriWixRoot) {
    $tauriWixTools = Get-ChildItem -LiteralPath $tauriWixRoot -Directory -Filter "WixTools*" -ErrorAction SilentlyContinue |
      Sort-Object @{ Expression = { if ($_.Name -match '^WixTools(\d+)$') { [int]$Matches[1] } else { -1 } }; Descending = $true }, @{ Expression = "Name"; Descending = $true }
    foreach ($toolDir in $tauriWixTools) {
      $tauriWixTool = Join-Path $toolDir.FullName $Name
      if (Test-Path $tauriWixTool) {
        return (Resolve-Path $tauriWixTool).Path
      }
    }
  }

  $cmd = Get-Command $Name -ErrorAction SilentlyContinue
  if ($cmd) {
    return $cmd.Source
  }

  throw "$Name not found. Run the Tauri MSI build once so a WiX tools directory is installed under $tauriWixRoot."
}

function Get-PackageVersion {
  $packageJson = Get-Content -LiteralPath (Join-Path $appRoot "package.json") -Raw | ConvertFrom-Json
  return $packageJson.version
}

function Get-MsiName {
  return "ListenerType_$(Get-PackageVersion)_x64_en-US.msi"
}

function Get-TauriMsiName {
  return "Listener Type_$(Get-PackageVersion)_x64_en-US.msi"
}

function Get-MsiPath {
  return Join-Path $releaseRoot "bundle\msi\$(Get-MsiName)"
}

function Get-TauriMsiPath {
  return Join-Path $releaseRoot "bundle\msi\$(Get-TauriMsiName)"
}

function Get-InstalledExePath {
  return "C:\Program Files\Listener Type\listener-type.exe"
}

function Find-BuiltMsiPath {
  foreach ($candidate in @((Get-MsiPath), (Get-TauriMsiPath))) {
    if (Test-Path $candidate) {
      return $candidate
    }
  }
  return Get-MsiPath
}

function Stop-RunningReleaseApp {
  $releaseExe = Join-Path $releaseRoot "listener-type.exe"
  $resolvedReleaseExe = $null
  if (Test-Path -LiteralPath $releaseExe) {
    $resolvedReleaseExe = (Resolve-Path -LiteralPath $releaseExe).Path
  }

  $running = @(Get-CimInstance Win32_Process -Filter "Name = 'listener-type.exe'" -ErrorAction SilentlyContinue)
  foreach ($process in $running) {
    $commandLine = [string]$process.CommandLine
    $matchesReleaseExe = $false
    if ($resolvedReleaseExe) {
      $matchesReleaseExe = $commandLine.IndexOf($resolvedReleaseExe, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
    }
    if (-not $matchesReleaseExe -and $commandLine.IndexOf($appRoot, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
      Write-Host "[info] Leaving unrelated listener-type.exe running: pid=$($process.ProcessId)"
      continue
    }

    Write-Host "[info] Stopping running Listener Type before MSI build: pid=$($process.ProcessId)"
    Stop-Process -Id $process.ProcessId -Force
  }
}

function Test-ReusableReleaseExe {
  $releaseExe = Join-Path $releaseRoot "listener-type.exe"
  if (-not (Test-Path -LiteralPath $releaseExe)) {
    throw "-ReuseExistingExe requires an existing release exe: $releaseExe"
  }

  $wixRoot = Join-Path $releaseRoot "wix\x64"
  foreach ($required in @(
      (Join-Path $wixRoot "main.wxs"),
      (Join-Path $wixRoot "locale.wxl"),
      (Join-Path $appRoot "src-tauri\wix\listener-type-ime-cleanup.wxs")
    )) {
    if (-not (Test-Path -LiteralPath $required)) {
      throw "-ReuseExistingExe requires an existing generated WiX bundle file: $required"
    }
  }

  $productPaths = @(
    "package.json",
    "package-lock.json",
    "index.html",
    "public",
    "src",
    "src-tauri\Cargo.toml",
    "src-tauri\Cargo.lock",
    "src-tauri\build.rs",
    "src-tauri\capabilities",
    "src-tauri\icons",
    "src-tauri\src",
    "src-tauri\tauri.conf.json",
    "src-tauri\wix",
    "windows-ime"
  )

  $dirty = @(& git -C $appRoot status --porcelain -- @productPaths)
  if ($dirty.Count -gt 0) {
    throw "-ReuseExistingExe refused because product inputs are dirty:`n$($dirty -join "`n")"
  }

  $latestEpochText = (& git -C $appRoot log -1 --format=%ct -- @productPaths) -join ""
  if (-not [string]::IsNullOrWhiteSpace($latestEpochText)) {
    $latestEpoch = [int64]$latestEpochText.Trim()
    $latestProductCommitUtc = [DateTimeOffset]::FromUnixTimeSeconds($latestEpoch).UtcDateTime
    $exeUtc = (Get-Item -LiteralPath $releaseExe).LastWriteTimeUtc
    if ($exeUtc -lt $latestProductCommitUtc) {
      throw ("-ReuseExistingExe refused because release exe is older than the latest product-input commit. exe={0:o} product_commit={1:o}" -f $exeUtc, $latestProductCommitUtc)
    }
  }

  Write-Host "[ok] Reusing existing release exe; product inputs are clean and newer docs/scripts-only commits do not require Rust rebuild."
}

function Test-WebView2Runtime {
  $paths = @(
    "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    "HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
  )
  foreach ($path in $paths) {
    if (Test-Path $path) {
      Write-Host "[ok] WebView2 Runtime registry key found"
      return
    }
  }
  Write-Warning "WebView2 Runtime registry key not found. Install Evergreen runtime if the app window is blank."
}

function Invoke-CmdWithHeartbeat {
  param(
    [Parameter(Mandatory = $true)][string]$Command,
    [Parameter(Mandatory = $true)][string]$Label,
    [int]$HeartbeatSeconds = 60,
    [int]$TimeoutSeconds = 3600
  )

  New-Item -ItemType Directory -Force -Path $ArtifactsRoot | Out-Null
  $safeLabel = ($Label -replace '[^A-Za-z0-9_.-]+', '_').Trim('_')
  if ([string]::IsNullOrWhiteSpace($safeLabel)) {
    $safeLabel = "cmd"
  }
  $logPath = Join-Path $ArtifactsRoot "$safeLabel.log"
  $cmdPath = Join-Path $ArtifactsRoot "$safeLabel.cmd"
  Remove-Item -LiteralPath $logPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $cmdPath -Force -ErrorAction SilentlyContinue

  function Write-LogDelta {
    param(
      [Parameter(Mandatory = $true)][string]$Path,
      [long]$Offset
    )

    if (-not (Test-Path -LiteralPath $Path)) {
      return $Offset
    }

    $stream = [System.IO.File]::Open(
      $Path,
      [System.IO.FileMode]::Open,
      [System.IO.FileAccess]::Read,
      [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
    )
    try {
      if ($stream.Length -le $Offset) {
        return $Offset
      }
      [void]$stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
      $length = [int]($stream.Length - $Offset)
      $buffer = New-Object byte[] $length
      $read = $stream.Read($buffer, 0, $length)
      if ($read -gt 0) {
        Write-Host -NoNewline ([System.Text.Encoding]::UTF8.GetString($buffer, 0, $read))
      }
      return $stream.Position
    } finally {
      $stream.Dispose()
    }
  }

  Set-Content -LiteralPath $cmdPath -Encoding ASCII -Value @(
    "@echo off",
    $Command,
    "exit /b %ERRORLEVEL%"
  )
  $commandWithLog = "call `"$cmdPath`" 1> `"$logPath`" 2>&1"
  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo.FileName = "cmd.exe"
  $process.StartInfo.UseShellExecute = $false
  $process.StartInfo.CreateNoWindow = $true
  $process.StartInfo.Arguments = "/d /c $commandWithLog"

  $started = Get-Date
  $lastHeartbeat = $started
  $deadline = $started.AddSeconds([Math]::Max(60, $TimeoutSeconds))
  $logOffset = 0L
  try {
    [void]$process.Start()
    while (-not $process.WaitForExit(1000)) {
      $logOffset = Write-LogDelta -Path $logPath -Offset $logOffset
      $now = Get-Date
      if ($now -ge $deadline) {
        $elapsed = [int]($now - $started).TotalSeconds
        Write-Warning "$Label timed out after $elapsed seconds; terminating process tree. Log: $logPath"
        & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
        $process.WaitForExit(5000) | Out-Null
        $logOffset = Write-LogDelta -Path $logPath -Offset $logOffset
        throw "$Label timed out after $elapsed seconds. See $logPath"
      }
      if (($now - $lastHeartbeat).TotalSeconds -ge $HeartbeatSeconds) {
        $elapsed = [int]($now - $started).TotalSeconds
        Write-Host "[info] $Label still running ($elapsed s elapsed); release builds can be quiet while rustc compiles a large crate."
        $lastHeartbeat = $now
      }
    }
    $process.WaitForExit()
    $logOffset = Write-LogDelta -Path $logPath -Offset $logOffset

    return $process.ExitCode
  } finally {
    $process.Dispose()
  }
}

function Invoke-MsvcBuild {
  param(
    [string]$VsDevCmd,
    [string]$CargoBin
  )

  $msiPath = Get-MsiPath
  Remove-Item -LiteralPath $msiPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath (Get-TauriMsiPath) -Force -ErrorAction SilentlyContinue

  $commandParts = @(
    "cd /d `"$appRoot`"",
    "call `"$VsDevCmd`" -arch=x64 -host_arch=x64",
    "set `"PATH=$CargoBin;%PATH%`"",
    "set `"PATH=%PATH:C:\Program Files\Git\usr\bin;=%`""
  )

  if ($CargoBuildJobs -gt 0) {
    $commandParts += "set `"CARGO_BUILD_JOBS=$CargoBuildJobs`""
    Write-Host "[info] Cargo build jobs forced to $CargoBuildJobs"
  } else {
    Write-Host "[info] Cargo build jobs left at Cargo default parallelism"
  }

  if ($UseSccache.IsPresent) {
    $sccache = Get-Command "sccache" -ErrorAction SilentlyContinue
    if (-not $sccache) {
      throw "-UseSccache was set, but sccache is not installed or not on PATH."
    }
    $commandParts += "set `"RUSTC_WRAPPER=$($sccache.Source)`""
    $commandParts += "set `"SCCACHE_IGNORE_SERVER_IO_ERROR=1`""
    Write-Host "[info] Using sccache rustc wrapper: $($sccache.Source)"
  }

  if ($IncrementalReleaseBuild.IsPresent) {
    $commandParts += "set `"CARGO_INCREMENTAL=1`""
    Write-Host "[info] CARGO_INCREMENTAL=1 enabled for this release build"
  }

  $commandParts += "npm.cmd run tauri build -- --target x86_64-pc-windows-msvc --bundles msi"
  $buildCommand = $commandParts -join " && "
  $buildStartedAt = Get-Date
  $exitCode = Invoke-CmdWithHeartbeat -Command $buildCommand -Label "Tauri Windows MSI build" -TimeoutSeconds $CommandTimeoutSeconds
  if ($exitCode -ne 0) {
    $payloadExe = Join-Path $releaseRoot "listener-type.exe"
    $payloadFresh = (Test-Path -LiteralPath $payloadExe) -and
      ((Get-Item -LiteralPath $payloadExe).LastWriteTime -ge $buildStartedAt.AddMinutes(-1))
    if (-not $payloadFresh) {
      throw "Tauri Windows MSI build failed with exit code $exitCode and the release payload was not rebuilt in this run; refusing to package a stale exe."
    }
    Write-Warning "Tauri Windows MSI build returned exit code $exitCode, but the release payload was rebuilt in this run. Trying to finish MSI linking from generated WiX objects."
    Repair-TauriMsiBundle
  }
}

function Repair-TauriMsiBundle {
  $wixRoot = Join-Path $releaseRoot "wix\x64"
  $mainSource = Join-Path $wixRoot "main.wxs"
  $mainObject = Join-Path $wixRoot "main.wixobj"
  $imeCleanupSource = Join-Path $appRoot "src-tauri\wix\listener-type-ime-cleanup.wxs"
  $imeCleanupObject = Join-Path $wixRoot "listener-type-ime-cleanup.wixobj"
  $locale = Join-Path $wixRoot "locale.wxl"
  $msiPath = Get-MsiPath

  foreach ($requiredPath in @($mainSource, $imeCleanupSource, $locale)) {
    if ([string]::IsNullOrWhiteSpace($requiredPath) -or -not (Test-Path $requiredPath)) {
      throw "Cannot repair Tauri MSI bundle because a required file is missing: $requiredPath"
    }
  }

  Enable-SameVersionMsiUpgrade -MainWxsPath $mainSource

  $candle = Find-WixTool "candle.exe"
  & $candle -nologo -arch x64 -out $mainObject $mainSource
  if ($LASTEXITCODE -ne 0) {
    throw "WiX candle.exe failed for main.wxs with exit code $LASTEXITCODE."
  }
  & $candle -nologo -arch x64 -out $imeCleanupObject $imeCleanupSource
  if ($LASTEXITCODE -ne 0) {
    throw "WiX candle.exe failed for listener-type-ime-cleanup.wxs with exit code $LASTEXITCODE."
  }

  $bundleDir = Split-Path -Parent $msiPath
  New-Item -ItemType Directory -Force -Path $bundleDir | Out-Null
  Remove-Item -LiteralPath $msiPath -Force -ErrorAction SilentlyContinue

  $light = Find-WixTool "light.exe"
  $suppressedIce = @(
    # Tauri's generated bootstrapper and same-version replacement MSI intentionally
    # trip these ICE checks. Keep the installer behavior stable and the release log clean.
    "-sice:ICE03",
    "-sice:ICE40",
    "-sice:ICE57",
    "-sice:ICE61"
  )
  & $light -nologo @suppressedIce -ext WixUIExtension -ext WixUtilExtension -loc $locale -out $msiPath $mainObject $imeCleanupObject
  if ($LASTEXITCODE -ne 0) {
    throw "WiX light.exe failed with exit code $LASTEXITCODE."
  }
  if (-not (Test-Path $msiPath)) {
    throw "WiX light.exe finished but MSI was not produced: $msiPath"
  }

  Write-Host "[ok] MSI linked from generated WiX objects -> $msiPath"
}

function Enable-SameVersionMsiUpgrade {
  param([string]$MainWxsPath)

  $text = Get-Content -LiteralPath $MainWxsPath -Raw
  if ($text -match 'AllowSameVersionUpgrades="yes"' -and $text -notmatch 'AllowDowngrades="yes"') {
    Write-Host "[ok] MSI same-version major upgrade already enabled"
    return
  }

  $next = [regex]::Replace(
    $text,
    '<MajorUpgrade\b([^>]*)\sAllowDowngrades="yes"([^>]*)/>',
    {
      param($match)
      $attributes = "$($match.Groups[1].Value)$($match.Groups[2].Value)"
      if ($attributes -notmatch '\bAllowSameVersionUpgrades=') {
        $attributes = "$attributes AllowSameVersionUpgrades=`"yes`""
      }
      if ($attributes -notmatch '\bDowngradeErrorMessage=') {
        $attributes = "$attributes DowngradeErrorMessage=`"A newer version of [ProductName] is already installed.`""
      }
      return "<MajorUpgrade$attributes />"
    },
    1)
  if ($next -eq $text) {
    throw "Cannot enable same-version MSI upgrade; generated MajorUpgrade element was not found in $MainWxsPath"
  }

  Set-Content -LiteralPath $MainWxsPath -Value $next -NoNewline
  Write-Host "[ok] MSI same-version major upgrade enabled in generated WiX"
}

function Reset-ArtifactsRoot {
  if (-not $CleanArtifacts) {
    New-Item -ItemType Directory -Force -Path $ArtifactsRoot | Out-Null
    return
  }

  $resolvedAppRoot = (Resolve-Path $appRoot).Path
  if (Test-Path $ArtifactsRoot) {
    $resolvedArtifactsRoot = (Resolve-Path $ArtifactsRoot).Path
    if (-not $resolvedArtifactsRoot.StartsWith($resolvedAppRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "-CleanArtifacts refuses to delete output outside the app root: $resolvedArtifactsRoot"
    }
    Remove-Item -LiteralPath $resolvedArtifactsRoot -Recurse -Force
  }
  New-Item -ItemType Directory -Force -Path $ArtifactsRoot | Out-Null
}

function Copy-WindowsArtifacts {
  $version = Get-PackageVersion
  $msiName = Get-MsiName
  $msiPath = Find-BuiltMsiPath
  $portableName = "ListenerType_${version}_x64_portable"
  $portableRoot = Join-Path $ArtifactsRoot $portableName
  $zipPath = Join-Path $ArtifactsRoot "$portableName.zip"

  if (-not (Test-Path $msiPath)) {
    throw "MSI not found: $msiPath"
  }

  Reset-ArtifactsRoot
  Copy-Item -LiteralPath $msiPath -Destination (Join-Path $ArtifactsRoot $msiName) -Force

  Remove-Item -LiteralPath $portableRoot -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $zipPath -Force -ErrorAction SilentlyContinue

  $hashPaths = @((Join-Path $ArtifactsRoot $msiName))
  if ($IncludePortable) {
    $exePath = Join-Path $releaseRoot "listener-type.exe"
    $webView2Loader = Get-ChildItem -Path (Join-Path $releaseRoot "build") -Recurse -Filter "WebView2Loader.dll" -ErrorAction SilentlyContinue |
      Where-Object { $_.FullName -match "\\out\\x64\\WebView2Loader\.dll$" } |
      Select-Object -First 1

    if (-not (Test-Path $exePath)) {
      throw "Release exe not found: $exePath"
    }
    if ($null -eq $webView2Loader) {
      throw "WebView2Loader.dll x64 not found under $releaseRoot\build"
    }

    New-Item -ItemType Directory -Force -Path $portableRoot | Out-Null
    Copy-Item -LiteralPath $exePath -Destination (Join-Path $portableRoot "listener-type.exe") -Force
    Copy-Item -LiteralPath $webView2Loader.FullName -Destination (Join-Path $portableRoot "WebView2Loader.dll") -Force
    Compress-Archive -LiteralPath $portableRoot -DestinationPath $zipPath -CompressionLevel Optimal
    $hashPaths += $zipPath
  }

  Write-Host ""
  Write-Host "Windows artifacts:"
  Get-ChildItem -File -LiteralPath $ArtifactsRoot | Select-Object Name,Length,LastWriteTime | Format-Table -AutoSize

  Write-Host "SHA256:"
  Get-FileHash -Algorithm SHA256 -LiteralPath $hashPaths | Select-Object Path,Hash | Format-List
}

function Stop-InstalledListenerType {
  $installedExePath = Get-InstalledExePath
  $resolvedInstalledExe = $null
  if (Test-Path -LiteralPath $installedExePath) {
    $resolvedInstalledExe = (Resolve-Path -LiteralPath $installedExePath).Path
  }
  if (-not $resolvedInstalledExe) {
    return
  }

  $running = @(Get-CimInstance Win32_Process -Filter "Name = 'listener-type.exe'" -ErrorAction SilentlyContinue)
  foreach ($process in $running) {
    $commandLine = [string]$process.CommandLine
    if ($commandLine.IndexOf($resolvedInstalledExe, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
      continue
    }
    if ($commandLine -match "(?i)--local-wake-helper") {
      Write-Host "[info] Leaving Listener Type local wake helper to exit with its parent: pid=$($process.ProcessId)"
      continue
    }
    if (-not (Get-Process -Id $process.ProcessId -ErrorAction SilentlyContinue)) {
      continue
    }

    Write-Host "[info] Requesting normal Listener Type shutdown before MSI update: pid=$($process.ProcessId)"
    Start-Process -FilePath $resolvedInstalledExe -ArgumentList "--quit" -WindowStyle Hidden | Out-Null
    $deadline = (Get-Date).AddSeconds(6)
    do {
      if (-not (Get-Process -Id $process.ProcessId -ErrorAction SilentlyContinue)) {
        Write-Host "[ok] Installed Listener Type shut down cleanly before MSI update"
        break
      }
      Start-Sleep -Milliseconds 150
    } while ((Get-Date) -lt $deadline)

    if (Get-Process -Id $process.ProcessId -ErrorAction SilentlyContinue) {
      Write-Warning "Installed Listener Type did not honor normal shutdown in 6 seconds; forcing stop for MSI compatibility: pid=$($process.ProcessId)"
      Stop-Process -Id $process.ProcessId -Force
    }
  }
}

function Assert-InstalledPayloadMatchesRelease {
  param([int]$TimeoutSeconds = 15)

  $releaseExePath = Join-Path $releaseRoot "listener-type.exe"
  $installedExePath = Get-InstalledExePath

  if (-not (Test-Path -LiteralPath $releaseExePath)) {
    throw "Release exe not found: $releaseExePath"
  }

  $releaseHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $releaseExePath).Hash
  $installedHash = ""
  $lastHashError = ""
  $deadline = (Get-Date).AddSeconds([Math]::Max(1, $TimeoutSeconds))
  do {
    if (Test-Path -LiteralPath $installedExePath) {
      try {
        $installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installedExePath).Hash
        $lastHashError = ""
        if ($releaseHash -eq $installedHash) {
          Write-Host "[ok] Installed Listener Type exe matches MSI payload"
          Write-Host "     exe: $installedExePath"
          Write-Host "     sha256: $installedHash"
          return
        }
      } catch {
        $lastHashError = $_.Exception.Message
      }
    }
    Start-Sleep -Milliseconds 250
  } while ((Get-Date) -lt $deadline)

  if (-not (Test-Path -LiteralPath $installedExePath)) {
    throw "Installed Listener Type exe not found after MSI update: $installedExePath"
  }

  if ($releaseHash -ne $installedHash) {
    $suffix = if ([string]::IsNullOrWhiteSpace($lastHashError)) { "" } else { " last_error=$lastHashError" }
    throw "Installed Listener Type exe hash does not match the freshly built MSI payload. release=$releaseHash installed=$installedHash$suffix"
  }
}

function Install-LatestMsi {
  $msiPath = Join-Path $ArtifactsRoot (Get-MsiName)
  if (-not (Test-Path -LiteralPath $msiPath)) {
    throw "Cannot install latest MSI because the copied artifact is missing: $msiPath"
  }

  Stop-InstalledListenerType

  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $logPath = Join-Path $ArtifactsRoot "msi-install-$stamp.log"
  Write-Host "[info] Installing latest Listener Type MSI through msiexec"
  Write-Host "       msi: $msiPath"
  Write-Host "       log: $logPath"
  $installCommand = "msiexec.exe /i `"$msiPath`" /qn /norestart /L*v `"$logPath`""
  $exitCode = Invoke-CmdWithHeartbeat -Command $installCommand -Label "msiexec install" -HeartbeatSeconds 20 -TimeoutSeconds $CommandTimeoutSeconds
  if ($exitCode -ne 0) {
    throw "msiexec install failed with code $exitCode. See $logPath"
  }

  Assert-InstalledPayloadMatchesRelease
}

function Start-InstalledListenerType {
  if (-not $LaunchInstalledApp.IsPresent) {
    return
  }

  Assert-InstalledPayloadMatchesRelease

  $installedExePath = (Resolve-Path -LiteralPath (Get-InstalledExePath)).Path
  $installRoot = Split-Path -Parent $installedExePath
  Write-Host "[info] Starting installed Listener Type app"
  Write-Host "       exe: $installedExePath"
  $launchedAt = Get-Date
  $process = Start-Process -FilePath $installedExePath -WorkingDirectory $installRoot -WindowStyle Hidden -PassThru
  $stabilityDeadline = (Get-Date).AddSeconds(5)
  do {
    Start-Sleep -Milliseconds 250
    $running = Get-CimInstance Win32_Process -Filter "ProcessId = $($process.Id)" -ErrorAction SilentlyContinue
    if (-not $running) {
      throw "Installed Listener Type process exited during the 5 second startup stability gate."
    }
  } while ((Get-Date) -lt $stabilityDeadline)

  $bluetoothCrashes = @(
    Get-WinEvent -FilterHashtable @{
      LogName = "Application"
      Id = 1000
      StartTime = $launchedAt.AddSeconds(-1)
    } -ErrorAction SilentlyContinue |
      Where-Object {
        $_.Message -match "listener-type\.exe" -and
        $_.Message -match "Windows\.Devices\.Bluetooth\.dll" -and
        $_.Message -match "0xc0000005"
      }
  )
  if ($bluetoothCrashes.Count -gt 0) {
    throw "Installed Listener Type produced a Windows.Devices.Bluetooth.dll 0xc0000005 crash during startup."
  }

  $commandLine = [string]$running.CommandLine
  if ($commandLine.IndexOf($installedExePath, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
    throw "Started Listener Type process is not the installed Program Files exe: $commandLine"
  }

  Write-Host "[ok] Started installed Listener Type app pid=$($process.Id)"
}

function Update-DesktopShortcut {
  if ($SkipDesktopShortcut.IsPresent) {
    Write-Host "[info] Skipping desktop shortcut refresh"
    return
  }

  $releaseExePath = Join-Path $releaseRoot "listener-type.exe"
  if (-not (Test-Path -LiteralPath $releaseExePath)) {
    throw "Cannot update desktop shortcut; release exe not found: $releaseExePath"
  }

  $installedExePath = Get-InstalledExePath
  if (-not (Test-Path -LiteralPath $installedExePath)) {
    Write-Host "[info] Desktop shortcut not updated: install the MSI first so the shortcut targets the user-facing app, not the repo build output."
    return
  }

  $releaseHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $releaseExePath).Hash
  $installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installedExePath).Hash
  if ($releaseHash -ne $installedHash) {
    Write-Host "[info] Desktop shortcut not updated: installed Listener Type exe does not match the freshly built MSI payload."
    Write-Host "       release:   $releaseHash"
    Write-Host "       installed: $installedHash"
    Write-Host "       install the new MSI, then refresh the shortcut."
    return
  }

  $userDesktop = [Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory)
  if ([string]::IsNullOrWhiteSpace($userDesktop) -and -not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
    $userDesktop = Join-Path $env:USERPROFILE "Desktop"
  }
  $commonDesktop = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonDesktopDirectory)
  if ([string]::IsNullOrWhiteSpace($userDesktop) -or -not (Test-Path -LiteralPath $userDesktop)) {
    throw "Cannot update desktop shortcut; Desktop directory not found."
  }

  $resolvedExe = (Resolve-Path -LiteralPath $installedExePath).Path
  $resolvedInstallRoot = Split-Path -Parent $resolvedExe
  $userShortcutPath = Join-Path $userDesktop "Listener Type.lnk"
  $commonShortcutPath = if (-not [string]::IsNullOrWhiteSpace($commonDesktop) -and (Test-Path -LiteralPath $commonDesktop)) {
    Join-Path $commonDesktop "Listener Type.lnk"
  } else {
    ""
  }
  # Tauri's MSI owns the common-desktop shortcut. Reuse it so an MSI update
  # does not leave a second, indistinguishable link on the user's desktop.
  $shortcutPath = if (-not [string]::IsNullOrWhiteSpace($commonShortcutPath) -and (Test-Path -LiteralPath $commonShortcutPath -PathType Leaf)) {
    $commonShortcutPath
  } else {
    $userShortcutPath
  }
  $desiredIconLocation = "$resolvedExe,0"
  $desiredDescription = "Listener Type installed application"
  $shell = New-Object -ComObject WScript.Shell
  $shortcutExists = Test-Path -LiteralPath $shortcutPath
  $shortcut = $shell.CreateShortcut($shortcutPath)
  $shortcut.TargetPath = $resolvedExe
  $shortcut.WorkingDirectory = $resolvedInstallRoot
  $shortcut.IconLocation = $desiredIconLocation
  $shortcut.Description = $desiredDescription
  $shortcut.Save()

  # MSI updates may remove a previously valid link. Always save and reread it.
  $refreshedShortcut = $shell.CreateShortcut($shortcutPath)
  $shortcutVerified =
    (Test-Path -LiteralPath $shortcutPath -PathType Leaf) -and
    [System.String]::Equals([string]$refreshedShortcut.TargetPath, $resolvedExe, [System.StringComparison]::OrdinalIgnoreCase) -and
    [System.String]::Equals([string]$refreshedShortcut.WorkingDirectory, $resolvedInstallRoot, [System.StringComparison]::OrdinalIgnoreCase) -and
    [System.String]::Equals([string]$refreshedShortcut.IconLocation, $desiredIconLocation, [System.StringComparison]::OrdinalIgnoreCase) -and
    [System.String]::Equals([string]$refreshedShortcut.Description, $desiredDescription, [System.StringComparison]::Ordinal)
  if (-not $shortcutVerified) {
    throw "Desktop shortcut verification failed after refresh: $shortcutPath"
  }

  if ($shortcutPath -ne $userShortcutPath -and (Test-Path -LiteralPath $userShortcutPath -PathType Leaf)) {
    $userShortcut = $shell.CreateShortcut($userShortcutPath)
    $isScriptManagedDuplicate =
      [System.String]::Equals([string]$userShortcut.TargetPath, $resolvedExe, [System.StringComparison]::OrdinalIgnoreCase) -and
      [System.String]::Equals([string]$userShortcut.Description, $desiredDescription, [System.StringComparison]::Ordinal)
    if ($isScriptManagedDuplicate) {
      Remove-Item -LiteralPath $userShortcutPath -Force
      Write-Host "[ok] Removed duplicate user desktop shortcut -> $userShortcutPath"
    }
  }

  if ($shortcutExists) {
    Write-Host "[ok] Desktop shortcut refreshed -> $shortcutPath"
  } else {
    Write-Host "[ok] Desktop shortcut created -> $shortcutPath"
  }
  Write-Host "     target: $resolvedExe"
}

Push-Location $appRoot
try {
  Write-Host "[info] App root: $appRoot"
  Install-RustMsvcToolchain
  Test-WebView2Runtime

  $vsDevCmd = Find-VsDevCmd
  Write-Host "[ok] VsDevCmd.bat -> $vsDevCmd"

  if (-not (Test-Command "node") -or -not (Test-Command "npm.cmd")) {
    throw "Node.js/npm.cmd not found. Install Node.js before packaging."
  }

  if ($SkipNpmCi) {
    if (-not (Test-Path (Join-Path $appRoot "node_modules"))) {
      throw "-SkipNpmCi was set, but node_modules does not exist."
    }
    Write-Host "[info] Skipping npm.cmd ci"
  } else {
    $exitCode = Invoke-CmdWithHeartbeat -Command "npm.cmd ci" -Label "npm ci" -HeartbeatSeconds 30 -TimeoutSeconds $CommandTimeoutSeconds
    if ($exitCode -ne 0) {
      throw "npm.cmd ci failed with exit code $exitCode."
    }
  }

  $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  Write-Host "[info] Default Windows package does not bundle or register the optional TSF IME."
  if ($UseSccache.IsPresent -and -not (Test-Command "sccache")) {
    throw "-UseSccache was set, but sccache is not installed or not on PATH."
  }
  if ($InstallMsi.IsPresent -or $LaunchInstalledApp.IsPresent) {
    Write-Host "[info] Install validation requested; stopping installed Listener Type before the long MSI build"
    Stop-InstalledListenerType
  }
  Stop-RunningReleaseApp
  if ($ReuseExistingExe.IsPresent) {
    Test-ReusableReleaseExe
  } else {
    Invoke-MsvcBuild -VsDevCmd $vsDevCmd -CargoBin $cargoBin
  }
  Repair-TauriMsiBundle
  Copy-WindowsArtifacts
  if ($InstallMsi.IsPresent) {
    Install-LatestMsi
  } elseif ($LaunchInstalledApp.IsPresent) {
    throw "-LaunchInstalledApp requires -InstallMsi so validation uses a freshly updated user-facing installation."
  }
  Update-DesktopShortcut
  Start-InstalledListenerType
} finally {
  Pop-Location
}
