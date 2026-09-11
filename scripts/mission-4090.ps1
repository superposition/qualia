# Step 28 -- the 4090 end-to-end mission.
#
# Run the zero-motion stack in simulation with the connectome prior loaded,
# open one exploration mission through the broker, close it, and check the four
# observable facts Step 28 names:
#
#   1. `GET /braid` shows `open_missions: 1` and then `0`;
#   2. a sealed MCAP segment exists;
#   3. `qualia-session` lists one session carrying the mission id;
#   4. the mission id / generation in the braid view are what the console's
#      Mission view and the watch Braid line render (both read `GET /braid`).
#
# The stack is launched through the repo's own path -- `qualia run --manifest
# config/stack-manifest.zero-motion.json`, i.e. `qualia-init` reading the
# manifest -- never a second supervisor. The mission is delivered to
# `POST /mission-control/envelopes` as the `qualia.mission-envelope.v1` wire
# contract, exactly as the agent's mission_broker tests do, with the agent
# started by the same manifest.
#
# Host safety (docs/agents.md section Host safety, D-011/D-013/D-014): the compute
# service opens a CUDA context, so this run is GPU work -- announce it and keep
# it serial. The run is bounded by `-RecorderSeconds` and the envelope's own
# deadline, and one build at a time with `-j 2`.

[CmdletBinding()]
param(
    # The exploration mission id. It names the MCAP segment, the session store
    # row and the braid's session, so it is restricted to characters both the
    # session id and the mission identifier accept.
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$')]
    [string]$MissionId = 'explore-frontier-1',

    # The stack manifest to run. The ticket names the zero-motion simulation
    # stack; a rehearsal may point at a scratch copy.
    [string]$Manifest = 'config/stack-manifest.zero-motion.json',

    [int]$WebPort = 18443,
    [int]$ComputePort = 46329,
    [int]$RecorderSeconds = 20,
    [int]$TimeoutSeconds = 60,

    # Seconds to keep the mission open before cancelling it, so a manual
    # console/TUI look has time to see `open missions: 1`.
    [int]$HoldOpenSeconds = 0,

    [string]$ScratchRoot = 'C:\tmp\qualia-mission-4090',

    [switch]$SkipBuild,
    [switch]$KeepRunning
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot

$release = Join-Path $repoRoot 'target\release'
$qualia = Join-Path $release 'qualia.exe'
$sessionBin = Join-Path $release 'qualia-session.exe'
$inspectBin = Join-Path $release 'qualia-mcap-inspect.exe'
$manifestPath = if ([IO.Path]::IsPathRooted($Manifest)) { $Manifest } else { Join-Path $repoRoot $Manifest }

$scratch = Join-Path $ScratchRoot $MissionId
$mcapRoot = Join-Path $scratch 'arena'
$logDir = Join-Path $scratch 'logs'
$tlsDir = Join-Path $scratch 'tls'
$store = Join-Path $scratch 'qualia_session_store.sqlite'
$journal = Join-Path $scratch 'mission-control.jsonl'
$sock = Join-Path $scratch 'control.sock'
$sealed = Join-Path $mcapRoot "$MissionId.mcap"

$token = 'mission-4090-broker-' + [Guid]::NewGuid().ToString('N')
$agentUrl = "https://127.0.0.1:$WebPort"

$checks = [ordered]@{}
$evid = [ordered]@{}
$stack = $null
$failure = $null

function Step([string]$message) {
    Write-Host "`n== $message"
}

function Invoke-CurlJson {
    param(
        [Parameter(Mandatory)][string]$Method,
        [Parameter(Mandatory)][string]$Url,
        [string]$Token,
        [string]$JsonBody
    )
    $bodyFile = Join-Path $scratch 'request.json'
    $outFile = Join-Path $scratch 'response.json'
    if (Test-Path -LiteralPath $outFile) { Remove-Item -LiteralPath $outFile -Force }
    $curlArgs = @('-ksS', '--max-time', '10', '-X', $Method, '-o', $outFile, '-w', '%{http_code}')
    if ($Token) { $curlArgs += @('-H', "Authorization: Bearer $Token") }
    if ($JsonBody) {
        [IO.File]::WriteAllText($bodyFile, $JsonBody)
        $curlArgs += @('-H', 'Content-Type: application/json', '--data-binary', "@$bodyFile")
    }
    $curlArgs += $Url
    $status = & curl.exe @curlArgs
    if ($LASTEXITCODE -ne 0) { throw "curl $Method $Url exited $LASTEXITCODE" }
    $content = if (Test-Path -LiteralPath $outFile) { Get-Content -LiteralPath $outFile -Raw } else { '' }
    [pscustomobject]@{ Status = [int]$status; Content = $content }
}

function Wait-Braid([string]$Url, [scriptblock]$Ready, [int]$Timeout, [string]$What) {
    $deadline = (Get-Date).AddSeconds($Timeout)
    $last = $null
    while ((Get-Date) -lt $deadline) {
        try {
            $reply = Invoke-CurlJson -Method GET -Url "$Url/braid"
            if ($reply.Status -eq 200) {
                $last = $reply.Content | ConvertFrom-Json
                if (& $Ready $last) { return $last }
            }
        }
        catch {
            # The agent is still coming up; keep polling until the deadline.
        }
        Start-Sleep -Milliseconds 200
    }
    throw "timed out after ${Timeout}s waiting for $What; last braid: $($last | ConvertTo-Json -Compress)"
}

function New-Envelope([string]$Mission, [string]$Key, [int]$Sequence, [string]$Command) {
    $issued = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $envelope = [ordered]@{
        schema_version  = 'qualia.mission-envelope.v1'
        broker_id       = 'mission-4090'
        producer_epoch  = 1
        sequence        = $Sequence
        mission_id      = $Mission
        idempotency_key = $Key
        command         = $Command
        issued_at_ms    = $issued
        deadline_ms     = $issued + 60000
        objective       = [ordered]@{
            kind        = 'explore_frontier'
            summary     = 'sweep the reachable frontier of the zero-motion arena'
            target_x_m  = $null
            target_y_m  = $null
            tolerance_m = $null
        }
        constraints     = [ordered]@{
            operating_area     = [ordered]@{ frame_id = 'odom'; min_x_m = -3.0; min_y_m = -3.0; max_x_m = 3.0; max_y_m = 3.0 }
            speed_ceiling_mps  = 0.15
            max_distance_m     = 2.0
            max_runtime_ms     = 30000
            max_replans        = 2
            evidence_max_age_ms = 2000
        }
        evidence_refs   = @('evidence:mission-4090-run')
        fly_governed    = $false
    }
    return ($envelope | ConvertTo-Json -Depth 8 -Compress)
}

function Stop-Stack {
    if (-not $stack -or $stack.HasExited) { return }
    Write-Host 'qualia: stopping the stack over the control socket'
    & $qualia stop --manifest $manifestPath | Write-Host
    if (-not $stack.WaitForExit(15000)) {
        Write-Warning "qualia-init did not exit within 15s (pid $($stack.Id))"
    }
}

try {
    Step "mission $MissionId on the 4090 (manifest $Manifest)"

    if (-not $SkipBuild) {
        Step 'build (cargo -j 2, one package set at a time)'
        & cargo build --release -j 2 `
            -p qualia-cli -p qualia-init -p qualia-agent -p qualia-explore `
            -p qualia-arena-recorder -p qualia-session -p qualia-mcap -p qualia-health `
            -p qualia-cuda-service
        if ($LASTEXITCODE -ne 0) { throw "cargo build exited $LASTEXITCODE" }
    }

    foreach ($bin in @($qualia, $sessionBin, $inspectBin)) {
        if (-not (Test-Path -LiteralPath $bin)) { throw "missing binary $bin (build it first)" }
    }
    if (-not (Test-Path -LiteralPath $manifestPath)) { throw "missing manifest $manifestPath" }
    $prior = Join-Path $repoRoot 'assets\brain\prior'
    foreach ($artifact in @('graph.bin', 'manifest.json')) {
        if (-not (Test-Path -LiteralPath (Join-Path $prior $artifact))) {
            throw "missing prior artifact $prior\$artifact (Step 8's build)"
        }
    }

    New-Item -ItemType Directory -Force -Path $mcapRoot, $logDir, $tlsDir | Out-Null
    if (Test-Path -LiteralPath $sealed) { Remove-Item -LiteralPath $sealed -Force }

    Step 'pre-create the session the recorder will seal into'
    $sessionManifest = [ordered]@{
        path          = $sealed.Replace('\', '/')
        filename      = "$MissionId.mcap"
        media_kind    = 'mcap'
        analysis_kind = 'raw_observation'
        status        = 'ready'
        duration_sec  = 0
        streams       = @(
            [ordered]@{
                key        = 'arena_mcap'
                kind       = 'mcap'
                role       = 'evidence'
                path       = $sealed.Replace('\', '/')
                sync_group = 'arena'
            }
        )
    }
    $sessionManifestPath = Join-Path $scratch 'session.json'
    [IO.File]::WriteAllText($sessionManifestPath, ($sessionManifest | ConvertTo-Json -Depth 6))
    $imported = & $sessionBin --store $store import-manifest --manifest $sessionManifestPath | Out-String
    if ($LASTEXITCODE -ne 0) { throw "qualia-session import-manifest exited $LASTEXITCODE" }
    $sessionId = ($imported | ConvertFrom-Json).session.id
    Write-Host "session id $sessionId -> $sealed"

    Step 'start the stack'
    $env:QUALIA_SOCK_PATH = $sock
    $env:QUALIA_LOG_DIR = $logDir
    $env:QUALIA_WEB_PORT = "$WebPort"
    $env:QUALIA_AGENT_URL = $agentUrl
    $env:QUALIA_COMPUTE_SOCKET = "127.0.0.1:$ComputePort"
    $env:QUALIA_CUDA_SM = '89'
    $env:QUALIA_MISSION_BROKER_TOKEN = $token
    $env:QUALIA_MISSION_CONTROL_JOURNAL = $journal
    $env:QUALIA_SESSION_STORE = $store
    $env:QUALIA_SESSION_ID = "$sessionId"
    $env:QUALIA_TLS_DIR = $tlsDir
    $env:QUALIA_MCAP_ROOT = $mcapRoot
    $env:QUALIA_ARENA_SESSION = $MissionId
    $env:QUALIA_MCAP_DURATION_SECONDS = "$RecorderSeconds"

    $stack = Start-Process -FilePath $qualia -ArgumentList @('run', '--manifest', $manifestPath) `
        -PassThru -NoNewWindow `
        -RedirectStandardOutput (Join-Path $logDir 'driver-init.out.log') `
        -RedirectStandardError (Join-Path $logDir 'driver-init.err.log')

    $braidStart = Wait-Braid $agentUrl { param($doc) $true } $TimeoutSeconds 'the agent to answer GET /braid'
    Write-Host ("braid at start: " + ($braidStart | ConvertTo-Json -Compress))

    Step 'open one exploration mission'
    $opened = Invoke-CurlJson -Method POST -Url "$agentUrl/mission-control/envelopes" -Token $token `
        -JsonBody (New-Envelope $MissionId "$MissionId-start" 1 'start')
    if ($opened.Status -ne 202) { throw "mission start delivery answered $($opened.Status): $($opened.Content)" }
    Write-Host $opened.Content

    $braidOpen = Wait-Braid $agentUrl { param($doc) $doc.open_missions -eq 1 } $TimeoutSeconds 'open_missions: 1'
    Write-Host ("braid with the mission open: " + ($braidOpen | ConvertTo-Json -Compress))
    if ($HoldOpenSeconds -gt 0) {
        Write-Host "holding the mission open for ${HoldOpenSeconds}s (manual front-end look)"
        Start-Sleep -Seconds $HoldOpenSeconds
    }

    Step 'close the mission (cancel; the broker verifies a zero stop)'
    $closed = Invoke-CurlJson -Method POST -Url "$agentUrl/mission-control/envelopes" -Token $token `
        -JsonBody (New-Envelope $MissionId "$MissionId-cancel" 2 'cancel')
    if ($closed.Status -ne 202) { throw "mission cancel delivery answered $($closed.Status): $($closed.Content)" }

    $braidClosed = Wait-Braid $agentUrl { param($doc) $doc.open_missions -eq 0 } $TimeoutSeconds 'open_missions: 0'
    Write-Host ("braid with the mission closed: " + ($braidClosed | ConvertTo-Json -Compress))

    $missions = (Invoke-CurlJson -Method GET -Url "$agentUrl/mission-control/missions").Content | ConvertFrom-Json
    $record = @($missions.missions | Where-Object { $_.envelope.mission_id -eq $MissionId })[0]
    Write-Host ("mission record: " + ($record | ConvertTo-Json -Depth 6 -Compress))

    if (-not $KeepRunning) {
        Step "wait for the recorder's bounded run to seal (${RecorderSeconds}s)"
        $sealedDeadline = (Get-Date).AddSeconds($RecorderSeconds + 30)
        while (-not (Test-Path -LiteralPath $sealed) -and (Get-Date) -lt $sealedDeadline) {
            Start-Sleep -Milliseconds 250
        }
        Step 'stop the stack'
        Stop-Stack
    }

    Step 'check the sealed MCAP segment'
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while (-not (Test-Path -LiteralPath $sealed) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }
    $partial = "$sealed.partial"
    if (Test-Path -LiteralPath $partial) { throw "recorder left a partial segment: $partial" }
    if (-not (Test-Path -LiteralPath $sealed)) { throw "no sealed segment at $sealed" }
    $inspection = & $inspectBin $sealed | Out-String
    if ($LASTEXITCODE -ne 0) { throw "qualia-mcap-inspect exited $LASTEXITCODE" }
    Write-Host $inspection

    Step 'check qualia-session'
    $listing = & $sessionBin --store $store list | Out-String
    if ($LASTEXITCODE -ne 0) { throw "qualia-session list exited $LASTEXITCODE" }
    Write-Host $listing
    $detail = & $sessionBin --store $store show --session-id $sessionId | Out-String
    if ($LASTEXITCODE -ne 0) { throw "qualia-session show exited $LASTEXITCODE" }
    Write-Host $detail
    $sessions = @($listing | ConvertFrom-Json)
    $shown = $detail | ConvertFrom-Json

    Step 'check the four observables'
    $checks['braid_open_missions_1'] = ($braidOpen.open_missions -eq 1)
    $checks['braid_open_missions_0'] = ($braidClosed.open_missions -eq 0)
    $checks['braid_session_is_mission_id'] = ($braidOpen.session_id -eq $MissionId) -and ($braidClosed.session_id -eq $MissionId)
    $checks['mission_terminal'] = ($record.status -eq 'cancelled') -and ($record.stage -eq 'terminal')
    $checks['sealed_segment_exists'] = (Test-Path -LiteralPath $sealed)
    $checks['sealed_segment_reads'] = ($inspection -match 'qualia\.mcap-inspection\.v1|"messages"')
    $checks['one_session'] = ($sessions.Count -eq 1)
    $checks['session_carries_mission_id'] = ($sessions[0].path -match [regex]::Escape($MissionId)) -and ($sessions[0].filename -match [regex]::Escape($MissionId))
    $checks['session_has_sealed_evidence'] = @($shown.streams | Where-Object { $_.stream_key -eq 'arena_mcap' -and $_.metadata_json -match 'sha256' }).Count -eq 1

    $evid = [ordered]@{
        schema_version   = 'qualia.mission-4090-evidence.v1'
        mission_id       = $MissionId
        manifest         = $Manifest
        web_port         = $WebPort
        agent_url        = $agentUrl
        braid_open       = $braidOpen
        braid_closed     = $braidClosed
        mission_record   = $record
        sealed_segment   = $sealed
        inspection      = ($inspection | ConvertFrom-Json -ErrorAction SilentlyContinue)
        session_list     = ($listing | ConvertFrom-Json)
        session_count    = $sessions.Count
        session_detail   = $shown
        checks           = $checks
    }
    $evidencePath = Join-Path $scratch 'evidence.json'
    [IO.File]::WriteAllText($evidencePath, ($evid | ConvertTo-Json -Depth 10))
    Write-Host "`nevidence: $evidencePath"
    Write-Host ($evid | ConvertTo-Json -Depth 10)

    $failed = @($checks.GetEnumerator() | Where-Object { -not $_.Value } | ForEach-Object { $_.Key })
    if ($failed.Count -gt 0) { throw "observable checks failed: $($failed -join ', ')" }
    Write-Host "`nmission: OK"
}
catch {
    $failure = $_
    Write-Host "`nmission: FAILED -- $($_.Exception.Message)" -ForegroundColor Red
}
finally {
    if (-not $KeepRunning) {
        try { Stop-Stack } catch { Write-Warning "stop failed: $($_.Exception.Message)" }
    }
}

if ($failure) {
    Write-Host "recorder log tail:"
    $recorderLog = Join-Path $logDir 'qualia-arena-recorder.log'
    if (Test-Path -LiteralPath $recorderLog) { Get-Content -LiteralPath $recorderLog -Tail 20 | Write-Host }
    exit 1
}
exit 0
