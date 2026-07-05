[CmdletBinding(PositionalBinding = $false)]
param(
    [ValidateSet("List", "ClickConnect")]
    [string]$Action = "List",
    [string]$TargetName = "listener",
    [int]$TimeoutSeconds = 8,
    [int]$MaxElements = 3000,
    [string]$OutputPath = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$started = Get-Date
$deadline = $started.AddSeconds([Math]::Max(1, $TimeoutSeconds))
$targetPattern = [regex]::Escape($TargetName)
$interestingPattern = "(?i)$targetPattern|listener|blistener|蓝牙|配对|添加设备|找到新设备|连接|connect|pair|bluetooth|swift pair"
$buttonPattern = "(?i)^连接$|^允许$|^配对$|connect|pair|allow"

function Get-ElementText {
    param([System.Windows.Automation.AutomationElement]$Element)
    try {
        $current = $Element.Current
        return [PSCustomObject]@{
            name = [string]$current.Name
            automation_id = [string]$current.AutomationId
            class_name = [string]$current.ClassName
            control_type = [string]$current.ControlType.ProgrammaticName
            enabled = [bool]$current.IsEnabled
            offscreen = [bool]$current.IsOffscreen
            rect = [string]$current.BoundingRectangle
        }
    } catch {
        return $null
    }
}

function Test-TextInteresting {
    param([object]$Info)
    if ($null -eq $Info) {
        return $false
    }
    return (($Info.name, $Info.automation_id, $Info.class_name, $Info.control_type) -join " ") -match $interestingPattern
}

function Get-ParentText {
    param(
        [System.Windows.Automation.AutomationElement]$Element,
        [int]$Levels = 5
    )
    $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
    $parts = [System.Collections.Generic.List[string]]::new()
    $node = $Element
    for ($i = 0; $i -lt $Levels -and $null -ne $node; $i++) {
        $info = Get-ElementText -Element $node
        if ($null -ne $info) {
            $parts.Add((($info.name, $info.automation_id, $info.class_name, $info.control_type) -join " ")) | Out-Null
        }
        try {
            $node = $walker.GetParent($node)
        } catch {
            break
        }
    }
    return ($parts -join " ")
}

function Get-UiSnapshot {
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $queue = [System.Collections.Generic.Queue[object]]::new()
    $queue.Enqueue([PSCustomObject]@{ element = $root; depth = 0 })
    $items = [System.Collections.Generic.List[object]]::new()
    $buttons = [System.Collections.Generic.List[object]]::new()
    $visited = 0

    while ($queue.Count -gt 0 -and $visited -lt $MaxElements -and (Get-Date) -lt $deadline) {
        $entry = $queue.Dequeue()
        $visited++
        $element = [System.Windows.Automation.AutomationElement]$entry.element
        $depth = [int]$entry.depth
        $info = Get-ElementText -Element $element
        if (Test-TextInteresting -Info $info) {
            $items.Add([PSCustomObject]@{
                depth = $depth
                name = $info.name
                automation_id = $info.automation_id
                class_name = $info.class_name
                control_type = $info.control_type
                enabled = $info.enabled
                offscreen = $info.offscreen
                rect = $info.rect
            }) | Out-Null
        }
        if ($null -ne $info -and $info.control_type -eq "ControlType.Button" -and $info.name -match $buttonPattern) {
            $context = Get-ParentText -Element $element
            $buttons.Add([PSCustomObject]@{
                element = $element
                name = $info.name
                context = $context
                target_context = ($context -match $interestingPattern)
            }) | Out-Null
        }
        if ($depth -lt 8) {
            try {
                $children = $element.FindAll(
                    [System.Windows.Automation.TreeScope]::Children,
                    [System.Windows.Automation.Condition]::TrueCondition)
                foreach ($child in $children) {
                    $queue.Enqueue([PSCustomObject]@{ element = $child; depth = $depth + 1 })
                }
            } catch {
                # Some shell surfaces disappear while notifications animate. Ignore and keep scanning.
            }
        }
    }

    return [PSCustomObject]@{
        scanned = $visited
        matches = @($items)
        buttons = @($buttons)
        timed_out = ((Get-Date) -ge $deadline)
    }
}

function Invoke-Button {
    param([System.Windows.Automation.AutomationElement]$Element)
    $patternObj = $null
    if ($Element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$patternObj)) {
        ([System.Windows.Automation.InvokePattern]$patternObj).Invoke()
        return "InvokePattern"
    }
    $legacyObj = $null
    if ($Element.TryGetCurrentPattern([System.Windows.Automation.LegacyIAccessiblePattern]::Pattern, [ref]$legacyObj)) {
        ([System.Windows.Automation.LegacyIAccessiblePattern]$legacyObj).DoDefaultAction()
        return "LegacyIAccessible"
    }
    throw "matched button has no supported invoke pattern"
}

$snapshot = Get-UiSnapshot
$clicked = $null
$errorText = $null

if ($Action -eq "ClickConnect") {
    $candidate = @($snapshot.buttons | Where-Object { $_.target_context } | Select-Object -First 1)
    if ($candidate.Count -gt 0) {
        try {
            $method = Invoke-Button -Element $candidate[0].element
            $clicked = [PSCustomObject]@{
                name = $candidate[0].name
                context = $candidate[0].context
                method = $method
            }
        } catch {
            $errorText = $_.Exception.Message
        }
    } else {
        $errorText = "no target-scoped connect/pair button found"
    }
}

$result = [PSCustomObject]@{
    generated_at = (Get-Date).ToString("o")
    action = $Action
    target_name = $TargetName
    scanned = $snapshot.scanned
    timed_out = $snapshot.timed_out
    match_count = @($snapshot.matches).Count
    button_count = @($snapshot.buttons).Count
    matches = @($snapshot.matches | Select-Object -First 60)
    candidate_buttons = @($snapshot.buttons | ForEach-Object {
        [PSCustomObject]@{
            name = $_.name
            target_context = $_.target_context
            context = $_.context
        }
    } | Select-Object -First 20)
    clicked = $clicked
    error = $errorText
}

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutputPath = Join-Path (Join-Path $PSScriptRoot "..\.cache\validation") "windows-ble-notification-$stamp.json"
}
if (-not [System.IO.Path]::IsPathRooted($OutputPath)) {
    $OutputPath = Join-Path (Join-Path $PSScriptRoot "..") $OutputPath
}
$outDir = Split-Path -Parent $OutputPath
if (-not [string]::IsNullOrWhiteSpace($outDir)) {
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null
}
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $OutputPath -Encoding UTF8

Write-Host ("result={0}" -f $(if ($clicked) { "CLICKED" } elseif ($Action -eq "List") { "LISTED" } else { "NOT_FOUND" }))
Write-Host ("output={0}" -f $OutputPath)
if ($errorText) {
    Write-Host ("error={0}" -f $errorText)
}
if ($Action -eq "ClickConnect" -and -not $clicked) {
    exit 2
}
