#requires -Version 7.0
param(
  [string]$LogPath = (Join-Path $env:LOCALAPPDATA 'Listener Type\Logs\listener-type.log'),
  [int]$SinceMinutes = 120,
  [string]$CandidateId = '',
  [switch]$CurrentFileOnly,
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

function Get-ObservationFingerprint([object]$Issue) {
  if ($Issue.kind) { return [string]$Issue.kind }
  $detail = [string]$Issue.detail
  if ($detail -match '(?i)panic|fatal|crash|watchdog') { return 'runtime_fatal_or_watchdog' }
  if ($detail -match '(?i)heartbeat.*timeout|timeout.*heartbeat') { return 'ble_heartbeat_timeout' }
  if ($detail -match '(?i)unexpected restart|automatic restart|reboot') { return 'unexpected_restart' }
  if ($detail -match '(?i)disconnect|connection lost') { return 'ble_disconnect_observation' }
  if ($Issue.reason) { return [string]$Issue.reason }
  return 'unclassified_runtime_observation'
}

function Get-ObservationAction([string]$Kind, [int]$Count) {
  $singleEvidenceCritical = @(
    'wake_gate_contradiction',
    'wake_candidate_stuck',
    'recording_stuck',
    'final_commit_truncated',
    'owner_text_destructive_reduction',
    'runtime_fatal_or_watchdog'
  )
  if ($Kind -in $singleEvidenceCritical) { return 'investigate_now' }
  if ($Count -ge 2) { return 'investigate_repeated' }
  return 'observe_more'
}

function Read-SharedLogSet([string]$Path, [bool]$OnlyCurrentFile) {
  if ($OnlyCurrentFile) { return @(Read-SharedLog $Path) }
  $parent = Split-Path -Parent $Path
  $leaf = Split-Path -Leaf $Path
  if (-not $parent -or -not (Test-Path -LiteralPath $parent -PathType Container)) {
    return @(Read-SharedLog $Path)
  }
  $escapedLeaf = [regex]::Escape($leaf)
  $files = @(
    Get-ChildItem -LiteralPath $parent -File | Where-Object {
      $_.Name -match "^${escapedLeaf}(?:\.(?<rotation>\d+))?$"
    } | Sort-Object {
      if ($_.Name -eq $leaf) { 0 } else { [int]([regex]::Match($_.Name, '\.(\d+)$').Groups[1].Value) }
    } -Descending
  )
  $combined = [System.Collections.Generic.List[string]]::new()
  foreach ($file in $files) {
    foreach ($line in (Read-SharedLog $file.FullName)) { $combined.Add($line) }
  }
  return @($combined)
}

function Select-CandidateLogScope([string[]]$Lines, [string]$ExpectedCandidateId) {
  $identities = [System.Collections.Generic.List[object]]::new()
  for ($index = 0; $index -lt $Lines.Count; $index++) {
    $line = $Lines[$index]
    if ($line -notmatch '\[build-identity\] desktop_version=(?<version>\S+) profile=(?<profile>\S+) candidate_id=(?<candidate>\S+) executable=(?<executable>.+)$') {
      continue
    }
    $identities.Add([ordered]@{
      line_index = $index
      desktop_version = $Matches.version
      profile = $Matches.profile
      candidate_id = $Matches.candidate
      executable = $Matches.executable.Trim()
    })
  }

  if (-not $ExpectedCandidateId) {
    return [ordered]@{ lines = $Lines; identity = $null; process_starts = 0; scope_found = $true }
  }

  $matching = @($identities | Where-Object candidate_id -eq $ExpectedCandidateId)
  if ($matching.Count -eq 0) {
    return [ordered]@{ lines = @(); identity = $null; process_starts = 0; scope_found = $false }
  }

  # The most recent contiguous run is authoritative. A later different build
  # proves that this candidate is no longer the process producing the log.
  $first = $matching[-1]
  while (
    $matching.Count -gt 1 -and
    $first.line_index -gt 0
  ) {
    $previous = $matching[@($matching).IndexOf($first) - 1]
    $differentBetween = @(
      $identities | Where-Object {
        $_.line_index -gt $previous.line_index -and
        $_.line_index -lt $first.line_index -and
        $_.candidate_id -ne $ExpectedCandidateId
      }
    ).Count -gt 0
    if ($differentBetween) { break }
    $first = $previous
    if ($first -eq $matching[0]) { break }
  }

  $nextDifferent = $identities | Where-Object {
    $_.line_index -gt $first.line_index -and $_.candidate_id -ne $ExpectedCandidateId
  } | Select-Object -First 1
  $endExclusive = if ($nextDifferent) { $nextDifferent.line_index } else { $Lines.Count }
  $scopedLines = if ($endExclusive -gt $first.line_index) {
    @($Lines[$first.line_index..($endExclusive - 1)])
  } else { @() }
  $starts = @(
    $identities | Where-Object {
      $_.line_index -ge $first.line_index -and
      $_.line_index -lt $endExclusive -and
      $_.candidate_id -eq $ExpectedCandidateId
    }
  )
  return [ordered]@{
    lines = $scopedLines
    identity = $starts[-1]
    process_starts = $starts.Count
    scope_found = $true
  }
}

function Convert-ListenerLog(
  [string[]]$Lines,
  [datetime]$CutoffUtc,
  [object]$BuildIdentity,
  [int]$ProcessStarts,
  [bool]$ScopeFound
) {
  $candidates = @{}
  $sessions = @{}
  $runtimeIssues = [System.Collections.Generic.List[object]]::new()
  $lastTimestamp = $null
  $activeEmbeddedId = $null
  $activeSessionId = $null

  foreach ($line in $Lines) {
    if ($line -notmatch '^(?<timestamp>\d{4}-\d{2}-\d{2}T\S+Z)') { continue }
    try { $timestamp = [datetime]::Parse($Matches.timestamp).ToUniversalTime() }
    catch { continue }
    if ($timestamp -lt $CutoffUtc) { continue }
    $lastTimestamp = $timestamp

    if ($line -match 'event=start embedded_session_id=(?<id>\d+) origin=(?<origin>\w+)') {
      $id = $Matches.id
      $origin = $Matches.origin
      $activeEmbeddedId = $id
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
          phrase_tail_to_capsule_ms = $null
          gate_compute_ms = $null
          phrase_signal = $null
          owner_matched = $null
          local_match_seen = $false
          local_absent_count = 0
          kws_hit_seen = $false
          best_phonetic_distance = $null
          max_transcript_chars = 0
          max_owner_score = $null
          local_helper_busy_count = 0
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
        if ($line -match 'local confirmation finished .* transcript_chars=(?<chars>\d+).* phonetic_best_distance=(?<distance>\d+)') {
          $candidate.max_transcript_chars = [math]::Max($candidate.max_transcript_chars, [int]$Matches.chars)
          $distance = [int]$Matches.distance
          if ($null -eq $candidate.best_phonetic_distance -or $distance -lt $candidate.best_phonetic_distance) {
            $candidate.best_phonetic_distance = $distance
          }
        }
        if ($line -match 'local confirmation.*(?:busy|local_wake_helper_busy)') {
          $candidate.local_helper_busy_count = [int]$candidate.local_helper_busy_count + 1
        }
        if ($line -match 'automatic streaming gate .* pcm_ms=(?<pcm>\d+).* total_compute_ms=(?<compute>\d+).* phrase_signal=(?<signal>\w+).* gate_decision=(?<decision>\w+).* enrolled_owner_matched=(?<owner>\w+)') {
          $candidate.pcm_ms = [int]$Matches.pcm
          $candidate.gate_compute_ms = [int]$Matches.compute
          $candidate.phrase_signal = $Matches.signal
          $candidate.decision = $Matches.decision.ToLowerInvariant()
          $candidate.owner_matched = $Matches.owner -eq 'true'
        }
        if ($line -match 'live automatic session activated .* wake_to_capsule_request_ms=(?<latency>\d+).* phrase_tail_to_capsule_ms=(?<tail>\d+)') {
          $candidate.decision = 'accept'
          $candidate.wake_to_capsule_ms = [int]$Matches.latency
          $candidate.phrase_tail_to_capsule_ms = [int]$Matches.tail
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

    if ($activeEmbeddedId -and $line -match '\[speaker-verification\] compared .* score=(?<score>\d+(?:\.\d+)?)') {
      $candidate = $candidates[$activeEmbeddedId]
      if ($candidate) {
        $score = [double]$Matches.score
        if ($null -eq $candidate.max_owner_score -or $score -gt $candidate.max_owner_score) {
          $candidate.max_owner_score = $score
        }
      }
    }

    if ($line -match '(?:session_id=(?<sid>[0-9a-fA-F-]{36})|"sessionId":"(?<frontsid>[0-9a-fA-F-]{36})")') {
      $sid = if ($Matches.sid) { $Matches.sid } else { $Matches.frontsid }
      $sid = $sid.ToLowerInvariant()
      $activeSessionId = $sid
      $session = Get-OrAdd $sessions $sid {
        [ordered]@{
          session_id = $sid
          recording_at = $null
          terminal_state = $null
          first_preview_at = $null
          first_preview_ms = $null
          first_preview_chars = $null
          first_preview_seq = $null
          first_preview_received_at = $null
          preview_delivery_ms = $null
          latest_recording_preview_chars = 0
          stop_preview_chars = $null
          post_stop_max_preview_chars = $null
          post_stop_growth_chars = 0
          post_stop_growth_ms = $null
          automatic_body_started_at = $null
          transcribing_at = $null
          stop_to_transcribing_ms = $null
          final_transcription_ms = $null
          done_at = $null
          stop_reason = $null
          auto_stop_count = 0
          endpoint_hold_reasons = [System.Collections.Generic.List[string]]::new()
          stop_to_done_ms = $null
          final_chars = $null
          insertion_status = $null
          user_stop = $null
          primary_chars = $null
          separated_chars = $null
          selected_chars = $null
          provider_target_chars = $null
          physical_overlap = $null
          sustained_non_target = $null
          degraded_owner_tail = $null
          wake_bound_primary_track = $false
          distinct_provider_rescue = $false
          noise_only_rescue = $false
          prefix_before_chars = $null
          prefix_after_chars = $null
        }
      }
      if ($line -match 'source=backend\.capsule event=emit_request .* state=(?<capsule_state>Recording|Transcribing) elapsed_ms=(?<elapsed>\d+) .* has_message=(?<has>true|false) message_chars=(?<chars>\d+)') {
        $elapsed = [int]$Matches.elapsed
        $chars = [int]$Matches.chars
        $capsuleState = $Matches.capsule_state
        if ($capsuleState -eq 'Recording' -and $elapsed -eq 0 -and -not $session.recording_at) {
          $session.recording_at = $timestamp.ToString('o')
        }
        if ($capsuleState -eq 'Transcribing' -and -not $session.transcribing_at) {
          $session.transcribing_at = $timestamp.ToString('o')
          $session.stop_preview_chars = $session.latest_recording_preview_chars
          if (-not $session.stop_reason) { $session.stop_reason = 'embedded_stop_boundary' }
        }
        if ($Matches.has -eq 'true' -and $chars -gt 0) {
          if ($capsuleState -eq 'Recording') {
            $session.latest_recording_preview_chars = [math]::Max(
              $session.latest_recording_preview_chars,
              $chars
            )
            if (-not $session.first_preview_at) {
              $session.first_preview_at = $timestamp.ToString('o')
              $session.first_preview_ms = $elapsed
              $session.first_preview_chars = $chars
              if ($line -match 'event=emit_request seq=(?<emitseq>\d+)') {
                $session.first_preview_seq = [int]$Matches.emitseq
              }
            }
          } elseif ($session.transcribing_at) {
            $session.post_stop_max_preview_chars = [math]::Max(
              [int]($session.post_stop_max_preview_chars ?? 0),
              $chars
            )
            $baseline = [int]($session.stop_preview_chars ?? 0)
            if ($chars -gt $baseline) {
              $session.post_stop_growth_chars = [math]::Max(
                $session.post_stop_growth_chars,
                $chars - $baseline
              )
              if ($null -eq $session.post_stop_growth_ms) {
                $stopAt = [datetime]::Parse($session.transcribing_at).ToUniversalTime()
                $session.post_stop_growth_ms = [math]::Max(
                  0,
                  [math]::Round(($timestamp - $stopAt).TotalMilliseconds)
                )
              }
            }
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
      if ($line -match 'stop_to_transcribing_ms=(?<elapsed>\d+) .* reason=(?<reason>\S+)') {
        $session.transcribing_at = $timestamp.ToString('o')
        $session.stop_to_transcribing_ms = [int]$Matches.elapsed
        $session.stop_reason = $Matches.reason
        $session.stop_preview_chars = $session.latest_recording_preview_chars
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
        $session.terminal_state = 'Done'
      }
      if ($line -match 'source=backend\.capsule event=emit_request .* state=Cancelled') {
        $session.terminal_state = 'Cancelled'
      }
    }


    if ($activeSessionId -and $sessions.ContainsKey($activeSessionId)) {
      $session = $sessions[$activeSessionId]
      if ($line -match '"event":"embedded_audio_final".*"timing_metric":"final_transcription_ms".*"timing_value_ms":(?<elapsed>\d+)') {
        $session.final_transcription_ms = [int]$Matches.elapsed
      }
      if ($line -match '\[embedded-ble\] target-speaker auto-stop sent session_id=(?<sid>[0-9a-fA-F-]{36})') {
        if ($Matches.sid.ToLowerInvariant() -eq $session.session_id) {
          $session.auto_stop_count = [int]$session.auto_stop_count + 1
        }
      }
      if ($line -match '\[asr\] target endpoint hold session_id=(?<sid>[0-9a-fA-F-]{36}) generation=(?<generation>\d+) reason=(?<reason>[a-zA-Z0-9_]+)') {
        if ($Matches.sid.ToLowerInvariant() -eq $session.session_id -and
            -not $session.endpoint_hold_reasons.Contains($Matches.reason)) {
          $session.endpoint_hold_reasons.Add($Matches.reason)
        }
      }
      if ($line -match '\[target-speaker\] awaiting owner-only final physical_overlap=(?<physical>true|false) sustained_non_target=(?<non_target>true|false) degraded_owner_tail=(?<tail>true|false)') {
        $session.physical_overlap = $Matches.physical -eq 'true'
        $session.sustained_non_target = $Matches.non_target -eq 'true'
        $session.degraded_owner_tail = $Matches.tail -eq 'true'
      }
      if ($line -match '\[target-speaker\] wake-bound single provider owner track retained chars=(?<chars>\d+)') {
        $session.wake_bound_primary_track = $true
      }
      if ($line -match '\[target-speaker\] separated final lost owner coverage; using distinct provider target track target_chars=(?<separated>\d+) provider_target_chars=(?<provider>\d+)') {
        $session.separated_chars = [int]$Matches.separated
        $session.provider_target_chars = [int]$Matches.provider
        $session.distinct_provider_rescue = $true
      }
      if ($line -match '\[target-speaker\] noise-only separated final lost owner coverage; preserving certified primary owner track primary_chars=(?<primary>\d+) separated_chars=(?<separated>\d+)') {
        $session.primary_chars = [int]$Matches.primary
        $session.separated_chars = [int]$Matches.separated
        $session.noise_only_rescue = $true
      }
      if ($line -match '\[target-speaker\] owner-only final selected primary_chars=(?<primary>\d+) target_chars=(?<target>\d+)') {
        $session.primary_chars = [int]$Matches.primary
        $session.selected_chars = [int]$Matches.target
        if ($null -eq $session.separated_chars) { $session.separated_chars = [int]$Matches.target }
      }
      if ($line -match '\[wake-phrase\] removed automatic activation prefix from final transcript .* before_chars=(?<before>\d+) after_chars=(?<after>\d+)') {
        $session.prefix_before_chars = [int]$Matches.before
        $session.prefix_after_chars = [int]$Matches.after
      }
    }

    if ($line -match '\[(?<severity>ERROR|WARN)\]') {
      # A second -match replaces PowerShell's global $Matches table. Preserve
      # severity before classifying the detail text.
      $runtimeSeverity = [string]$Matches.severity
      if (
        $runtimeSeverity -eq 'ERROR' -or
        $line -match '(?i)panic|crash|unexpected restart|connection lost|disconnected|fatal'
      ) {
        $runtimeIssues.Add([ordered]@{
          severity = $runtimeSeverity.ToLowerInvariant()
          timestamp = $timestamp.ToString('o')
          reason = 'runtime_log'
          detail = $line
        })
      }
    }
  }

  $issues = [System.Collections.Generic.List[object]]::new()
  if (-not $ScopeFound) {
    $issues.Add([ordered]@{
      severity = 'error'
      kind = 'candidate_identity_not_found'
      candidate_id = $CandidateId
    })
  }
  if ($ProcessStarts -gt 1) {
    $issues.Add([ordered]@{
      severity = 'error'
      kind = 'desktop_process_restarted'
      process_starts = $ProcessStarts
    })
  }
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
    if (
      $candidate.decision -eq 'reject' -and
      $null -ne $candidate.best_phonetic_distance -and
      $candidate.best_phonetic_distance -le 2 -and
      $candidate.local_absent_count -ge 3 -and
      $null -ne $candidate.max_owner_score -and
      $candidate.max_owner_score -ge 0.38
    ) {
      $issues.Add([ordered]@{
        severity = 'error'
        kind = 'owner_wake_evidence_rejected'
        embedded_session_id = $candidate.embedded_session_id
        best_phonetic_distance = $candidate.best_phonetic_distance
        repeated_absent_count = $candidate.local_absent_count
        max_owner_score = $candidate.max_owner_score
      })
    }
    if ($candidate.local_helper_busy_count -ge 2) {
      $issues.Add([ordered]@{
        severity = 'warning'
        kind = 'wake_helper_contention'
        embedded_session_id = $candidate.embedded_session_id
        busy_count = $candidate.local_helper_busy_count
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
    # Candidate-start latency includes the time needed to physically say the
    # wake phrase.  The actionable UX latency is the wait after the detected
    # phrase tail; do not tune thresholds because someone spoke more slowly.
    if (
      $null -ne $candidate.phrase_tail_to_capsule_ms -and
      $candidate.phrase_tail_to_capsule_ms -gt 350
    ) {
      $issues.Add([ordered]@{
        severity = 'warning'
        kind = 'wake_after_phrase_tail_slow'
        embedded_session_id = $candidate.embedded_session_id
        value_ms = $candidate.phrase_tail_to_capsule_ms
        gate_compute_ms = $candidate.gate_compute_ms
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
    if ($null -ne $session.stop_to_transcribing_ms -and $session.stop_to_transcribing_ms -gt 1500) {
      $issues.Add([ordered]@{
        severity = 'warning'
        kind = 'endpoint_stop_dispatch_slow'
        session_id = $session.session_id
        value_ms = $session.stop_to_transcribing_ms
      })
    }
    if ($null -ne $session.final_transcription_ms -and $session.final_transcription_ms -gt 1500) {
      $issues.Add([ordered]@{
        severity = 'warning'
        kind = 'final_transcription_slow'
        session_id = $session.session_id
        value_ms = $session.final_transcription_ms
        stop_to_done_ms = $session.stop_to_done_ms
      })
    }
    if ($session.recording_at -and -not $session.done_at -and -not $session.terminal_state) {
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
    # A provider final may legitimately complete text after the stop boundary.
    # This is only a regression when the body itself starts after stop (the
    # historical wake-only race); ordinary final completion must not look like
    # swallowed text in the daily observer.
    $bodyStartedAfterStop = $false
    if ($session.automatic_body_started_at -and $session.transcribing_at) {
      try {
        $bodyStartedAfterStop =
          ([datetime]::Parse($session.automatic_body_started_at).ToUniversalTime() -gt
           [datetime]::Parse($session.transcribing_at).ToUniversalTime())
      } catch {
        $bodyStartedAfterStop = $false
      }
    }
    if ($session.post_stop_growth_chars -gt 0 -and $bodyStartedAfterStop) {
      $issues.Add([ordered]@{
        severity = 'error'
        kind = 'body_text_grew_after_stop'
        session_id = $session.session_id
        stop_reason = $session.stop_reason
        stop_preview_chars = $session.stop_preview_chars
        post_stop_max_preview_chars = $session.post_stop_max_preview_chars
        growth_chars = $session.post_stop_growth_chars
        growth_after_stop_ms = $session.post_stop_growth_ms
        body_started_after_stop = $true
      })
    }
    if ($session.auto_stop_count -gt 1) {
      $issues.Add([ordered]@{
        severity = 'error'
        kind = 'duplicate_target_auto_stop'
        session_id = $session.session_id
        count = $session.auto_stop_count
      })
    }
    $explicitOtherSpeaker = $session.sustained_non_target -eq $true -or $session.degraded_owner_tail -eq $true
    if (
      $null -ne $session.primary_chars -and
      $null -ne $session.selected_chars -and
      $session.primary_chars -gt 0 -and
      ($session.selected_chars / $session.primary_chars) -lt 0.80 -and
      -not $explicitOtherSpeaker -and
      -not $session.distinct_provider_rescue -and
      -not $session.noise_only_rescue
    ) {
      $issues.Add([ordered]@{
        severity = 'error'
        kind = 'owner_text_destructive_reduction'
        session_id = $session.session_id
        primary_chars = $session.primary_chars
        selected_chars = $session.selected_chars
        coverage = [math]::Round($session.selected_chars / $session.primary_chars, 3)
      })
    }
    if (
      $null -ne $session.prefix_after_chars -and
      $null -ne $session.final_chars -and
      $session.final_chars -lt $session.prefix_after_chars
    ) {
      $issues.Add([ordered]@{
        severity = 'error'
        kind = 'final_commit_truncated'
        session_id = $session.session_id
        expected_chars = $session.prefix_after_chars
        committed_chars = $session.final_chars
      })
    }
  }
  foreach ($issue in $runtimeIssues) { $issues.Add($issue) }

  # Daily-use policy: one weak observation is not a reason to patch product
  # behavior. Group repeated symptoms first; only destructive/catastrophic
  # evidence escalates from a single occurrence. This report never edits code.
  $observationGroups = @(
    $issues |
      Group-Object { Get-ObservationFingerprint $_ } |
      ForEach-Object {
        $kind = [string]$_.Name
        $count = $_.Count
        [ordered]@{
          kind = $kind
          occurrences = $count
          action = Get-ObservationAction $kind $count
          sample = $_.Group[0]
        }
      } |
      Sort-Object @{ Expression = {
        switch ($_.action) {
          'investigate_now' { 0 }
          'investigate_repeated' { 1 }
          default { 2 }
        }
      } }, @{ Expression = 'occurrences'; Descending = $true }, kind
  )
  $investigate = @($observationGroups | Where-Object action -ne 'observe_more')
  $watchlist = @($observationGroups | Where-Object action -eq 'observe_more')

  [ordered]@{
    schema = 'listener-runtime-health/v6'
    generated_at = [datetime]::UtcNow.ToString('o')
    log_path = $LogPath
    cutoff_utc = $CutoffUtc.ToString('o')
    candidate_scope = [ordered]@{
      requested_candidate_id = $CandidateId
      found = $ScopeFound
      process_starts = $ProcessStarts
      build_identity = $BuildIdentity
    }
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
    daily_observation = [ordered]@{
      policy = 'observe_first_no_automatic_fix'
      decision_rule = 'repeat_before_patch_except_destructive_or_fatal_evidence'
      status = if ($investigate.Count -gt 0) { 'investigate' } elseif ($watchlist.Count -gt 0) { 'observe' } else { 'stable_in_observed_window' }
      investigate = $investigate
      watchlist = $watchlist
      blind_spots = @(
        'physical_led_animation_requires_human_or_camera_evidence',
        'plugged_soft_off_requires_disconnect_hold_and_explicit_wake_evidence',
        'absence_of_a_log_issue_is_not_product_acceptance'
      )
    }
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
  $scope = Select-CandidateLogScope (Read-SharedLogSet $LogPath $CurrentFileOnly.IsPresent) $CandidateId
  $report = Convert-ListenerLog $scope.lines $cutoff $scope.identity $scope.process_starts $scope.scope_found
  Write-Report $report
  if (-not $Watch) { break }
  if ($TimeoutSeconds -gt 0 -and ((Get-Date) - $started).TotalSeconds -ge $TimeoutSeconds) { break }
  Start-Sleep -Milliseconds ([Math]::Max(200, $PollMilliseconds))
} while ($true)
