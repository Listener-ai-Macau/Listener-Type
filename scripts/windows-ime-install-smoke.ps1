param(
  [Parameter(Mandatory = $true)]
  [string]$InstallerPath,
  [Parameter(Mandatory = $true)]
  [ValidateSet("nsis", "msi")]
  [string]$InstallerKind,
  [switch]$SkipUninstall
)

$ErrorActionPreference = "Stop"

$TextServiceClsid = "{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}"
$ProfileGuid = "{19F96D43-A5EB-46C9-8A73-9FCA5A0630C8}"
$LangId = "0x00000804"
$KeyboardCategoryGuid = "{34745C63-B2F0-4784-8B67-5E12C8701A31}"
$ImmersiveCategoryGuid = "{13A016DF-560B-46CD-947A-4C3AF1E0E35D}"
$SystrayCategoryGuid = "{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}"

# Keep this script aligned with the backend status check and the TSF IPC path
# used by ListenerTypeImeSubmit-* named pipes.
$ExpectedBackendKeys = @(
  "Software\Classes\CLSID\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}\InprocServer32",
  "Software\WOW6432Node\Classes\CLSID\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}\InprocServer32",
  "Software\Microsoft\CTF\TIP\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}\LanguageProfile\0x00000804\{19F96D43-A5EB-46C9-8A73-9FCA5A0630C8}",
  "Software\Microsoft\CTF\TIP\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}\Category\Category\{34745C63-B2F0-4784-8B67-5E12C8701A31}\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}",
  "Software\Microsoft\CTF\TIP\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}\Category\Category\{13A016DF-560B-46CD-947A-4C3AF1E0E35D}\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}",
  "Software\Microsoft\CTF\TIP\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}\Category\Category\{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}"
)

function Test-IsAdministrator {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = [Security.Principal.WindowsPrincipal]::new($identity)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Join-ProcessArguments {
  param(
    [string[]]$ArgumentList = @()
  )

  $quoted = foreach ($argument in $ArgumentList) {
    if ($argument.Length -eq 0) {
      '""'
    } elseif ($argument -notmatch '[\s"]') {
      $argument
    } else {
      $escaped = $argument -replace '(\\*)"', '$1$1\"'
      $escaped = $escaped -replace '(\\+)$', '$1$1'
      '"' + $escaped + '"'
    }
  }
  return ($quoted -join " ")
}

function Invoke-CheckedProcess {
  param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,
    [string[]]$ArgumentList = @(),
    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  $commandLine = Join-ProcessArguments $ArgumentList
  Write-Host "[run] $Label`: $FilePath $commandLine"
  $process = Start-Process -FilePath $FilePath -ArgumentList $commandLine -Wait -PassThru
  if ($process.ExitCode -ne 0) {
    throw "$Label failed with exit code $($process.ExitCode)"
  }
}

function Open-LocalMachineSubKey {
  param(
    [Parameter(Mandatory = $true)]
    [Microsoft.Win32.RegistryView]$View,
    [Parameter(Mandatory = $true)]
    [string]$SubKey
  )

  $baseKey = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, $View)
  try {
    return $baseKey.OpenSubKey($SubKey)
  } finally {
    $baseKey.Dispose()
  }
}

function Assert-RegistryKey {
  param(
    [Parameter(Mandatory = $true)]
    [Microsoft.Win32.RegistryView]$View,
    [Parameter(Mandatory = $true)]
    [string]$SubKey,
    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  $key = Open-LocalMachineSubKey -View $View -SubKey $SubKey
  if ($null -eq $key) {
    throw "Missing $Label registry key ($View): HKLM\$SubKey"
  }
  $key.Close()
  Write-Host "[ok] $Label registry key present ($View)"
}

function Get-DefaultRegistryValue {
  param(
    [Parameter(Mandatory = $true)]
    [Microsoft.Win32.RegistryView]$View,
    [Parameter(Mandatory = $true)]
    [string]$SubKey,
    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  $key = Open-LocalMachineSubKey -View $View -SubKey $SubKey
  if ($null -eq $key) {
    throw "Missing $Label registry key ($View): HKLM\$SubKey"
  }
  try {
    $value = [string]$key.GetValue("")
    if ([string]::IsNullOrWhiteSpace($value)) {
      throw "$Label default registry value is empty ($View): HKLM\$SubKey"
    }
    return $value
  } finally {
    $key.Close()
  }
}

function Assert-ListenerTypeImeInstalled {
  $comKey = "Software\Classes\CLSID\$TextServiceClsid\InprocServer32"
  $x64Dll = Get-DefaultRegistryValue -View Registry64 -SubKey $comKey -Label "x64 COM"
  $x86Dll = Get-DefaultRegistryValue -View Registry32 -SubKey $comKey -Label "x86 COM"

  foreach ($dll in @($x64Dll, $x86Dll)) {
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf)) {
      throw "Registered IME DLL path does not exist: $dll"
    }
  }

  $installRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $x64Dll))
  $expectedX64 = Join-Path $installRoot "windows-ime\x64\ListenerTypeIme.dll"
  $expectedX86 = Join-Path $installRoot "windows-ime\x86\ListenerTypeIme.dll"
  if ($x64Dll -ne $expectedX64) {
    throw "x64 COM DLL path points outside the installed IME directory. Expected '$expectedX64', got '$x64Dll'"
  }
  if ($x86Dll -ne $expectedX86) {
    throw "x86 COM DLL path points outside the installed IME directory. Expected '$expectedX86', got '$x86Dll'"
  }
  if (-not (Test-Path -LiteralPath (Join-Path $installRoot "listener-type.exe") -PathType Leaf)) {
    throw "Installed Listener Type executable not found under $installRoot"
  }

  Assert-RegistryKey -View Registry64 -SubKey "Software\Microsoft\CTF\TIP\$TextServiceClsid\LanguageProfile\$LangId\$ProfileGuid" -Label "TSF language profile"
  Assert-RegistryKey -View Registry64 -SubKey "Software\Microsoft\CTF\TIP\$TextServiceClsid\Category\Category\$KeyboardCategoryGuid\$TextServiceClsid" -Label "TSF keyboard category"
  Assert-RegistryKey -View Registry64 -SubKey "Software\Microsoft\CTF\TIP\$TextServiceClsid\Category\Category\$ImmersiveCategoryGuid\$TextServiceClsid" -Label "TSF immersive category"
  Assert-RegistryKey -View Registry64 -SubKey "Software\Microsoft\CTF\TIP\$TextServiceClsid\Category\Category\$SystrayCategoryGuid\$TextServiceClsid" -Label "TSF systray category"

  foreach ($key in $ExpectedBackendKeys) {
    Assert-RegistryKey -View Registry64 -SubKey $key -Label "backend-required"
  }

  Write-Host "[ok] Windows IME backend would report installed"
  return $installRoot
}

function Uninstall-ListenerType {
  param(
    [Parameter(Mandatory = $true)]
    [string]$InstallRoot
  )

  if ($InstallerKind -eq "nsis") {
    $uninstaller = Join-Path $InstallRoot "uninstall.exe"
    if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
      throw "NSIS uninstaller not found: $uninstaller"
    }
    Invoke-CheckedProcess -FilePath $uninstaller -ArgumentList @("/S") -Label "NSIS uninstall"
  } else {
    Invoke-CheckedProcess -FilePath "msiexec.exe" -ArgumentList @("/x", $InstallerPath, "/qn", "/norestart") -Label "MSI uninstall"
  }
}

if (-not (Test-IsAdministrator)) {
  throw "Windows IME install smoke must run from an elevated Administrator PowerShell."
}

$InstallerPath = (Resolve-Path -LiteralPath $InstallerPath).Path
if ($InstallerKind -eq "nsis") {
  Invoke-CheckedProcess -FilePath $InstallerPath -ArgumentList @("/S", "/AllUsers") -Label "NSIS install"
} else {
  Invoke-CheckedProcess -FilePath "msiexec.exe" -ArgumentList @("/i", $InstallerPath, "/qn", "/norestart") -Label "MSI install"
}

$installRoot = Assert-ListenerTypeImeInstalled
if (-not $SkipUninstall) {
  Uninstall-ListenerType -InstallRoot $installRoot
}
