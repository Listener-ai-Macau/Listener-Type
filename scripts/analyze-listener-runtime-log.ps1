#requires -Version 7.0
param(
  [string]$LogPath = (Join-Path $env:LOCALAPPDATA 'Listener Type\Logs\listener-type.log'),
  [int]$SinceMinutes = 120,
  [string]$OutputPath = '',
  [switch]$Watch,
  [int]$PollMilliseconds = 1000,
  [int]$TimeoutSeconds = 0
)

$ErrorActionPreference = 'Stop'

function Read-SharedLog([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return @() }
  $stream = [System.IO.File]::Open(
    $Path,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read,
    [System.IO.FileShare]::ReadWrite
  )
  try {
    $reader = [System.IO.StreamReader]::new($stream)
    try { return @($reader.ReadToEnd() -split "`r?`n") }
    finally { $reader.Dispose() }
  } finally { $stream.Dispose() }
}

function Get-OrAdd([hashtable]$Table, [string]$Key, [scriptblock]$Factory) {
  if (-not $Table.ContainsKey($Key)) { $Table[$Key] = & $Factory }
  return $Table[$Key]
}

function Convert-ListenerLog([string[]]$Lines, [datetime]$CutoffUtc) {
  $candidates = @{}
  $sessions = @{}
  $runtimeIssues = [System.Collections.Generic.List[object]]::new()
  $lastTimestamp = $null

  foreach ($line in $Lines) {
    if ($line -notmatch '^(?<timestamp>\d{4}-\d{2}-\d{2}T\S+Z)') { continue }
    try { $timestamp = [datetime]::Parse($Matches.timestamp).ToUniversalTime() }
    catch { continue }
    if ($timestamp -lt $CutoffUtc) { continue }
    $lastTimestamp = $timestamp

    if ($line -match 'event=start embedded_session_id=(?<id>\d+) origin=(?<origin>\w+)') {
      $id = $Matches.id
      $origin = $Matches.origin
      foreach ($prior in $candidates.Values) {
        if ($prior.embedded_session_id -ne [int]$id -and $prior.decision -eq 'pending') {
          # Firmware/actor recovery can replace a VAD proposal without an
          # explicit STOP for the old proposal. A later start proves the actor
          # is not stuck; retain it as a diagnostic supersession, not an error.
          $prior.decision = 'superseded'
          $prior.stopped_at = $timestamp.ToString('o')
        }
      }
      $candidate = Get-OrAdd $candidates $id {
        [ordered]@{
          embedded_session_id = [int]$id
          origin = $origin
          started_at = $timestamp.ToString('o')
          stopped_at = $null
          decision = 'pending'
          reject_reason = $null
          pcm_ms = $null
          wake_to_capsule_ms = $null
          phrase_signal = $null
          owner_matched = $null
          local_match_seen = $false
          local_absent_count = 0
          kws_hit_seen = $false
        }
      }
      continue
    }

    if ($line -match 'embedded_session_id=(?<id>\d+)') {
      $id = $Matches.id
      if ($candidates.ContainsKey($id)) {
        $candidate = $candidates[$id]
        if ($line -match 'event=stop embedded_session_id=\d+') {
          $candidate.stopped_at = $timestamp.ToString('o')
        }
        if ($line -match 'stage1 KWS hit') { $candidate.kws_hit_seen = $true }
        if ($line -match 'stage2 local confirm finished .* matched=true') {
          $candidate.local_match_seen = $true
        }
        if ($line -match 'local-only Absent recorded .* count=(?<count>\d+)') {
          $candidate.local_absent_count = [int]$Matches.count
        }
        if ($line -match 'automatic streaming gate .* pcm_ms=(?<pcm>\d+).* phrase_signal=(?<signal>\w+).* gate_decision=(?<decision>\w+).* owner_matched=(?<owner>\w+)') {
          $candidate.pcm_ms = [int]$Matches.pcm
          $candidate.phrase_signal = $Matches.signal
          $candidate.decision = $Matches.decision.ToLowerInvariant()
          $candidate.owner_matched = $Matches.owner -eq 'true'
        }
        if ($line -match 'live automatic session activated .* wake_to_capsule_request_ms=(?<latency>\d+)') {
          $candidate.decision = 'accept'
          $candidate.wake_to_capsule_ms = [int]$Matches.latency
        }
        if ($line -match 'automatic candidate rejected .* reason=(?<reason>[a-zA-Z0-9_-]+)') {
          $candidate.decision = 'reject'
          $candidate.reject_reason = $Matches.reason
        } elseif ($line -match 'hidden automatic candidate rejected silently reason=(?<reason>[a-zA-Z0-9_-]+)') {
          $candidate.decision = 'reject'
          $candidate.reject_reason = $Matches.reason
        }
      }
    }

    if ($line -match '(?:session_id=(?<sid>[0-9a-fA-F-]{36})|"sessionId":"(?<frontsid>[0-9a-fA-F-]{36})")') {
      $sid = if ($Matches.sid) { $Matches.sid } else { $Matches.frontsid }
      $sid = $sid.ToLowerInvariant()
      $session = Get-OrAdd $sessions $sid {
        [ordered]@{
          session_id = $sid
          recording_at = $null
          first_preview_at = $null
          first_preview_ms = $null
          first_preview_chars = $null
          first_preview_seq = $null
          first_preview_received_at = $null
          preview_delivery_ms = $null
          automatic_body_started_at = $null
          transcribing_at = $null
          done_at = $null
          stop_reason = $null
          stop_to_done_ms = $null
          final_chars = $null
          insertion_status = $null
          user_stop = $null
        }
      }
      if ($line -match 'source=backend\.capsule event=emit_request .* state=Recording elapsed_ms=(?<elapsed>\d+) .* has_message=(?<has>true|false) message_chars=(?<chars>\d+)') {
        $elapsed = [int]$Matches.elapsed
        $chars = [int]$Matches.chars
        if ($elapsed -eq 0 -and -not $session.recording_at) {
          $session.recording_at = $timestamp.ToString('o')
        }
        if ($Matches.has -eq 'true' -and $chars -gt 0 -and -not $session.first_preview_at) {
          $session.first_preview_at = $timestamp.ToString('o')
          $session.first_preview_ms = $elapsed
          $session.first_preview_chars = $chars
          if ($line -match 'event=emit_request seq=(?<emitseq>\d+)') {
            $session.first_preview_seq = [int]$Matches.emitseq
          }
        }
      }
      if ($line -match '\[wake-phrase\] automatic body started after capsule') {
        $session.automatic_body_started_at = $timestamp.ToString('o')
      }
      if (
        $null -ne $session.first_preview_seq -and
        -not $session.first_preview_received_at -and
        $line -match 'source=frontend\.capsule event=event_received .*"seq":(?<frontseq>\d+)' -and
        [int]$Matches.frontseq -eq $session.first_preview_seq
      ) {
        $session.first_preview_received_at = $timestamp.ToString('o')
        $previewAt = [datetime]::Parse($session.first_preview_at).ToUniversalTime()
        $session.preview_delivery_ms = [math]::Max(
          0,
          [math]::Round(($timestamp - $previewAt).TotalMilliseconds)
        )
      }
      if ($line -match 'stop_to_transcribing_ms=\d+ .* reason=(?<reason>\S+)') {
        $session.transcribing_at = $timestamp.ToString('o')
        $session.stop_reason = $Matches.reason
      }
      if ($line -match 'stop_to_done_ms=(?<elapsed>\d+)') {
        $session.stop_to_done_ms = [int]$Matches.elapsed
      }
      if ($line -match 'final completion actions .* chars=(?<chars>\d+) insertion_status=(?<status>\w+).* user_stop=(?<user>true|false)') {
        $session.final_chars = [int]$Matches.chars
        $session.insertion_status = $Matches.status
        $session.user_stop = $Matches.user -eq 'true'
      }
      if ($line -match 'source=backend\.capsule event=emit_request .* state=Done') {
        $session.done_at = $timestamp.ToString('o')
      }
    }

    if ($line -match '\[(ERROR|WARN)\]') {
      $runtimeIssues.Add([ordered]@{
        severity = $Matches[1].ToLowerInvariant()
        timestamp = $timestamp.ToString('o')
        reason = 'runtime_log'
        detail = $line
      })
    }
  }

  $issues = [System.Collections.Generic.List[object]]::new()
  foreach ($candidate in $candidates.Values) {
    # A VoiceActivation candidate is only a VAD proposal. Rejecting ordinary
    # owner/room speech without the wake phrase is expected false-wake
    # suppression, not a runtime failure. Escalate only the contradictory case
    # where both phrase and enrolled owner evidence were present yet the gate
    # still rejected it.
    if (
      $candidate.origin -eq 'VoiceActivation' -and
      $candidate.decision -eq 'reject' -and
      $candidate.owner_matched -eq $true -and
      ($candidate.local_match_seen -or $candidate.kws_hit_seen)
    ) {
      $issues.Add([ordered]@{
        severity = 'error'
        kind = 'wake_gate_contradiction'
        embedded_session_id = $candidate.embedded_session_id
        detail = "phrase=$($candidate.phrase_signal) owner=$($candidate.owner_matched) local_absent=$($candidate.local_absent_count) pcm_ms=$($candidate.pcm_ms)"
      })
    }
    if ($candidate.decision -eq 'pending' -and $candidate.started_at) {
      $startedAt = [datetime]::Parse($candidate.started_at)
      if (($lastTimestamp - $startedAt).TotalSeconds -gt 8) {
        $issues.Add([ordered]@{
          severity = 'error'
          kind = 'wake_candidate_stuck'
          embedded_session_id = $candidate.embedded_session_id
          age_ms = [math]::Round(($lastTimestamp - $startedAt).TotalMilliseconds)
        })
      }
    }
    if ($null -ne $candidate.wake_to_capsule_ms -and $candidate.wake_to_capsule_ms -gt 1200) {
      $issues.Add([ordered]@{
        severity = 'warning'
        kind = 'wake_capsule_slow'
        embedded_session_id = $candidate.embedded_session_id
        value_ms = $candidate.wake_to_capsule_ms
      })
    }
  }
  foreach ($session in $sessions.Values) {
    # recording->first-preview contains the time in which the user is still
    # saying the wake phrase and the first body words. It is not UI latency.
    # Measure only the actionable path: provider/body text emitted by the
    # backend until the matching capsule event is received by the frontend.
    if ($null -ne $session.preview_delivery_ms -and $session.preview_delivery_ms -gt 80) {
      $issues.Add([ordered]@{
        severity = 'warning'
        kind = 'preview_delivery_slow'
        session_id = $session.session_id
        value_ms = $session.preview_delivery_ms
      })
    }
    if ($null -ne $session.stop_to_done_ms -and $session.stop_to_done_ms -gt 1500) {
      $issues.Add([ordered]@{
        severity = 'warning'
        kind = 'stop_to_done_slow'
        session_id = $session.session_id
        value_ms = $session.stop_to_done_ms
      })
    }
    if ($session.recording_at -and -not $session.done_at) {
      $recordingAt = [datetime]::Parse($session.recording_at)
      if (($lastTimestamp - $recordingAt).TotalSeconds -gt 8) {
        $issues.Add([ordered]@{
          severity = 'error'
          kind = 'recording_stuck'
          session_id = $session.session_id
          age_ms = [math]::Round(($lastTimestamp - $recordingAt).TotalMilliseconds)
        })
      }
    }
  }
  foreach ($issue in $runtimeIssues) { $issues.Add($issue) }

  [ordered]@{
    schema = 'listener-runtime-health/v3'
    generated_at = [datetime]::UtcNow.ToString('o')
    log_path = $LogPath
    cutoff_utc = $CutoffUtc.ToString('o')
    candidates = @($candidates.Values | Sort-Object embedded_session_id)
    candidate_summary = [ordered]@{
      accepted = @($candidates.Values | Where-Object decision -eq 'accept').Count
      suppressed_without_phrase = @(
        $candidates.Values | Where-Object {
          $_.decision -eq 'reject' -and -not $_.local_match_seen -and -not $_.kws_hit_seen
        }
      ).Count
      rejected_after_phrase_evidence = @(
        $candidates.Values | Where-Object {
          $_.decision -eq 'reject' -and ($_.local_match_seen -or $_.kws_hit_seen)
        }
      ).Count
      superseded = @($candidates.Values | Where-Object decision -eq 'superseded').Count
      pending = @($candidates.Values | Where-Object decision -eq 'pending').Count
    }
    sessions = @($sessions.Values | Where-Object { $_.recording_at } | Sort-Object recording_at)
    issues = @($issues)
  }
}

function Write-Report([object]$Report) {
  $json = $Report | ConvertTo-Json -Depth 8
  if ($OutputPath) {
    $parent = Split-Path -Parent $OutputPath
    if ($parent) { [System.IO.Directory]::CreateDirectory($parent) | Out-Null }
    [System.IO.File]::WriteAllText($OutputPath, $json, [System.Text.UTF8Encoding]::new($false))
  } else {
    $json
  }
}

$started = Get-Date
do {
  $cutoff = [datetime]::UtcNow.AddMinutes(-[Math]::Max(1, $SinceMinutes))
  $report = Convert-ListenerLog (Read-SharedLog $LogPath) $cutoff
  Write-Report $report
  if (-not $Watch) { break }
  if ($TimeoutSeconds -gt 0 -and ((Get-Date) - $started).TotalSeconds -ge $TimeoutSeconds) { break }
  Start-Sleep -Milliseconds ([Math]::Max(200, $PollMilliseconds))
} while ($true)
