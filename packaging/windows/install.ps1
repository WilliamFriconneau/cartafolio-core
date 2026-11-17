<#
MemCP on Windows: the node (`memcp serve`) and the bridge (`memcp-capture`) as tasks of your user, started at logon and
again after a crash, with their logs. Nothing needs an administrator and nothing is put outside your own folders.

  powershell -ExecutionPolicy Bypass -File install.ps1               install or update
  powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall    remove the tasks and the programs; the memory is kept

What goes where:
  %LOCALAPPDATA%\MemCP\bin      memcp.exe, memcp-capture.exe, memcp-janitor.exe
  %LOCALAPPDATA%\MemCP\data     the memory (a folder of databases: back it up, it is yours)
  %LOCALAPPDATA%\MemCP\bridge   the bridge's settings, its key (readable by you only) and what waits to be sent
  %LOCALAPPDATA%\MemCP\logs     node.log, bridge.log, transcribe.log
  Task Scheduler, folder \MemCP  the tasks MemCP node, MemCP bridge, MemCP transcribe

There is no Unix socket on Windows: assistants reach the memory through the bridge on this machine,
`memcp-capture connect 127.0.0.1:PORT`, and know neither its address nor its key.
#>
[CmdletBinding()]
param(
    [switch]$Yes,
    [string]$Version = $env:MEMCP_VERSION,
    # Take memcp.exe, memcp-capture.exe and memcp-janitor.exe from this folder (a build of your own) instead of a release.
    [string]$From = $env:MEMCP_FROM,
    [string]$Language = $(if ($env:MEMCP_LANGUAGE) { $env:MEMCP_LANGUAGE } else { 'en' }),
    [string]$Spaces = $(if ($env:MEMCP_SPACES) { $env:MEMCP_SPACES } else { 'personal,work,health' }),
    [int]$Port = $(if ($env:MEMCP_PORT) { [int]$env:MEMCP_PORT } else { 8791 }),
    [int]$BridgePort = $(if ($env:MEMCP_BRIDGE_PORT) { [int]$env:MEMCP_BRIDGE_PORT } else { 8792 }),
    # Voice notes read on this machine by whisper.cpp (whisper-cli.exe and ffmpeg.exe on the PATH) with this model,
    [string]$WhisperModel = $env:MEMCP_WHISPER_MODEL,
    # or by a speech service of the OpenAI kind (key in MEMCP_SPEECH_API_KEY): the recordings leave this machine for it.
    [string]$SpeechApi = $env:MEMCP_SPEECH_API,
    # Install the programs and the memory, leave nothing running.
    [switch]$NoTasks,
    [switch]$Uninstall,
    # With -Uninstall: also delete the memory and the logs.
    [switch]$Purge,
    [string]$Root = $(if ($env:MEMCP_ROOT) { $env:MEMCP_ROOT } else { Join-Path $env:LOCALAPPDATA 'MemCP' })
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

$Repo = if ($env:MEMCP_REPO) { $env:MEMCP_REPO } else { 'WilliamFriconneau/memcp-core' }
$Bin = Join-Path $Root 'bin'
$Data = Join-Path $Root 'data'
$Bridge = Join-Path $Root 'bridge'
$Logs = Join-Path $Root 'logs'
$TaskPath = '\MemCP\'
$Tasks = @('MemCP node', 'MemCP bridge', 'MemCP transcribe')
$Programs = @('memcp', 'memcp-capture', 'memcp-janitor')
$Target = 'x86_64-pc-windows-msvc'
$NodeUrl = "http://127.0.0.1:$Port"
$Connection = 'desktop-bridge'

function Say($text) { Write-Host $text }
function Step($text) { Write-Host ''; Write-Host $text -ForegroundColor Cyan }
function Die($text) { Write-Host "error: $text" -ForegroundColor Red; exit 1 }
function Ask($question, $default) {
    if ($Yes -or -not [Environment]::UserInteractive) { return $default }
    $a = Read-Host "$question [$default]"
    if ([string]::IsNullOrWhiteSpace($a)) { $default } else { $a }
}

# Only the person may read: inheritance cut, full control for this user alone.
function Make-Private($path) {
    & icacls $path /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { Die "cannot restrict $path to this user" }
}
function Make-PrivateFile($path) {
    & icacls $path /inheritance:r /grant:r "$($env:USERNAME):F" /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { Die "cannot restrict $path to this user" }
}

function Stop-Tasks {
    foreach ($t in $Tasks) {
        $task = Get-ScheduledTask -TaskPath $TaskPath -TaskName $t -ErrorAction SilentlyContinue
        if ($task) {
            Stop-ScheduledTask -TaskPath $TaskPath -TaskName $t -ErrorAction SilentlyContinue
            Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $t -Confirm:$false
        }
    }
    # A program the task started can outlive the task's stop: the ones of this install only.
    Get-Process -Name $Programs -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($Bin, [StringComparison]::OrdinalIgnoreCase) } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

# --- uninstall -----------------------------------------------------------------------------------------------
if ($Uninstall) {
    Step 'Removing MemCP from this computer'
    Stop-Tasks
    foreach ($d in @($Bin, $Bridge)) { if (Test-Path $d) { Remove-Item -Recurse -Force $d } }
    Say 'the tasks, the programs and the bridge (its key with it) are removed.'
    if ($Purge) {
        $ok = $Yes -or ((Ask "Delete the memory in $Data and the logs? This cannot be undone (y/n)" 'n') -match '^(y|yes|o|oui)$')
        if ($ok) {
            foreach ($d in @($Data, $Logs)) { if (Test-Path $d) { Remove-Item -Recurse -Force $d } }
            if ((Test-Path $Root) -and -not (Get-ChildItem $Root)) { Remove-Item $Root }
            Say 'the memory and the logs are deleted.'
        } else { Say "the memory is kept in $Data." }
    } else {
        Say "your memory is kept in $Data (delete it yourself, or run again with -Purge)."
    }
    exit 0
}

Step 'MemCP for Windows'
Say 'A durable memory for your AI assistants, kept on this computer.'

# --- choices -------------------------------------------------------------------------------------------------
Step '1/5  Choices'
$Language = Ask 'Language of search and of the page (en or fr)?' $Language
$Spaces = Ask 'First spaces, comma separated (a space is one box for one part of your life)' $Spaces
$Port = [int](Ask 'Port of the node on this computer?' $Port)
$BridgePort = [int](Ask 'Port of the bridge on this computer, for assistants?' $BridgePort)
if ($Port -eq $BridgePort) { Die 'the node and the bridge need two ports' }
$NodeUrl = "http://127.0.0.1:$Port"
$BridgeUrl = "$NodeUrl/mcp/$Connection"

# --- the programs --------------------------------------------------------------------------------------------
Step '2/5  Programs'
foreach ($d in @($Root, $Bin, $Data, $Bridge, $Logs)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }
Make-Private $Root
Stop-Tasks
if ($From) {
    foreach ($p in $Programs) {
        $f = Join-Path $From "$p.exe"
        if (-not (Test-Path $f)) { Die "$f is missing" }
        Copy-Item -Force $f (Join-Path $Bin "$p.exe")
    }
    Say "taken from $From"
} else {
    if (-not $Version) {
        $Version = (Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest").tag_name
        if (-not $Version) { Die 'cannot find the latest release (are you online?)' }
    }
    $base = "https://github.com/$Repo/releases/download/$Version"
    Say "release $Version for $Target"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("memcp-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $sums = (Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS").Content
        if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
        foreach ($p in $Programs) {
            $f = "$p-$Version-$Target.exe"
            Invoke-WebRequest -UseBasicParsing "$base/$f" -OutFile (Join-Path $tmp $f)
            $line = ($sums -split "`n") | Where-Object { $_.Trim().EndsWith(" $f") } | Select-Object -First 1
            if (-not $line) { Die "$f is not in SHA256SUMS" }
            $want = ($line.Trim() -split '\s+')[0].ToLower()
            $got = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $f)).Hash.ToLower()
            if ($want -ne $got) { Die "checksum mismatch for ${f}: refusing to install it" }
        }
        Say 'checksums match.'
        foreach ($p in $Programs) { Copy-Item -Force (Join-Path $tmp "$p-$Version-$Target.exe") (Join-Path $Bin "$p.exe") }
    } finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
}
$Memcp = Join-Path $Bin 'memcp.exe'
$MemcpBridge = Join-Path $Bin 'memcp-capture.exe'
Say "installed in $Bin"

# --- the memory ----------------------------------------------------------------------------------------------
Step '3/5  Your memory'
if (Test-Path (Join-Path $Data 'config.toml')) {
    Say "$Data already holds a memory: left as it is."
} else {
    & $Memcp --data $Data init --spaces $Spaces --language $Language
    if ($LASTEXITCODE -ne 0) { Die 'the memory could not be made' }
}

# --- the tasks -----------------------------------------------------------------------------------------------
# A task of this user, at logon, without a window: `cmd /c` sends the program's lines to its log. Started again
# after a failure, never stopped for running long.
function Register-MemcpTask($name, $program, [string[]]$arguments, $log, [int]$everyMinutes = 0) {
    $quoted = ($arguments | ForEach-Object { if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ } }) -join ' '
    $command = "/d /c `"`"$program`" $quoted >> `"$log`" 2>&1`""
    $action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\cmd.exe" -Argument $command -WorkingDirectory $Root
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
    if ($everyMinutes -gt 0) {
        $trigger.Repetition = (New-ScheduledTaskTrigger -Once -At (Get-Date) `
            -RepetitionInterval (New-TimeSpan -Minutes $everyMinutes)).Repetition
    }
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable `
        -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) `
        -MultipleInstances IgnoreNew
    # S4U: runs without a window and without storing a password; it reaches this machine only, which is all it needs.
    $principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType S4U -RunLevel Limited
    try {
        Register-ScheduledTask -TaskPath $TaskPath -TaskName $name -Action $action -Trigger $trigger -Settings $settings `
            -Principal $principal -Force | Out-Null
    } catch {
        # Where S4U is not allowed: a task of the open session (a console window may show, minimised).
        $principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
        Register-ScheduledTask -TaskPath $TaskPath -TaskName $name -Action $action -Trigger $trigger -Settings $settings `
            -Principal $principal -Force | Out-Null
    }
}

Step '4/5  The node and the bridge''s key'
if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
    Die "port $Port is taken by another program; choose another with -Port"
}
Register-MemcpTask 'MemCP node' $Memcp @('--data', $Data, 'serve', '--listen', "127.0.0.1:$Port") (Join-Path $Logs 'node.log')
$nodeProcess = $null
if ($NoTasks) {
    $nodeProcess = Start-Process -FilePath $Memcp -ArgumentList @('--data', "`"$Data`"", 'serve', '--listen', "127.0.0.1:$Port") `
        -WindowStyle Hidden -PassThru
} else {
    Start-ScheduledTask -TaskPath $TaskPath -TaskName 'MemCP node'
}
$up = $false
foreach ($i in 1..30) {
    try { Invoke-RestMethod "$NodeUrl/health" -TimeoutSec 2 | Out-Null; $up = $true; break } catch { Start-Sleep 1 }
}
if (-not $up) { Die "the node does not answer on ${NodeUrl}: see $(Join-Path $Logs 'node.log')" }

$console = (& $Memcp --data $Data console-token owner 2>$null | Select-String -Pattern 'hcpc_[A-Za-z0-9_]+' |
    Select-Object -First 1).Matches.Value
if (-not $console) { Die 'no console key could be made' }
$headers = @{ Authorization = "Bearer $console"; 'X-Requested-With' = 'memcp' }

# This computer joins its own memory as a device, as a phone does: that makes the person's « À trier », where every
# capture goes. Its key writes only to À trier and reads nothing; it is kept beside the bridge's, readable by you only.
$deviceKey = Join-Path $Bridge 'device.key'
if (-not (Test-Path $deviceKey)) {
    $code = (Invoke-RestMethod -Method Post -Uri "$NodeUrl/admin/api/pairing" -Headers $headers -ContentType 'application/json' -Body '{}').code
    if (-not $code) { Die 'the node gave no pairing code' }
    $paired = Invoke-RestMethod -Method Post -Uri "$NodeUrl/pair" -ContentType 'application/json' `
        -Body (@{ code = $code; device = 'this-computer' } | ConvertTo-Json -Compress)
    if (-not $paired.key) { Die 'no key in the pairing''s answer' }
    Set-Content -NoNewline -Encoding ascii -Path $deviceKey -Value $paired.key
    Make-PrivateFile $deviceKey
    Say 'this computer is paired with its memory: its captures go to À trier.'
}

# The bridge's own connection: the memory's tools for the assistants here, and the voice notes to read.
$bridgeKey = Join-Path $Bridge 'bridge.key'
if (Test-Path $bridgeKey) {
    Say 'the bridge already has its key.'
} else {
    $out = & $Memcp --data $Data connect $Connection --all --public-url $NodeUrl 2>&1
    if ($LASTEXITCODE -ne 0) { $out = & $Memcp --data $Data connect $Connection --all --rotate --public-url $NodeUrl 2>&1 }
    $key = ($out | Select-String -Pattern 'hcp_[A-Za-z0-9_-]+' | Select-Object -First 1).Matches.Value
    if (-not $key) { Die "no key could be made for the bridge: $out" }
    Set-Content -NoNewline -Encoding ascii -Path $bridgeKey -Value $key
    Make-PrivateFile $bridgeKey
    $key = $null
    Say "a key for the bridge is in $bridgeKey (readable by you only)."
}

$conf = @(
    '# Written by install.ps1.',
    "url = $BridgeUrl",
    "key_file = $bridgeKey",
    "outbox = $(Join-Path $Bridge 'outbox')"
)
if ($WhisperModel -or $SpeechApi) {
    $conf += 'transcribe = owner-a-trier'
    $conf += "whisper_lang = $Language"
}
if ($WhisperModel) {
    if (-not (Test-Path $WhisperModel)) { Die "no model at $WhisperModel" }
    $whisper = (Get-Command whisper-cli -ErrorAction SilentlyContinue).Source
    $ffmpeg = (Get-Command ffmpeg -ErrorAction SilentlyContinue).Source
    if (-not $whisper -or -not $ffmpeg) { Die 'whisper-cli.exe and ffmpeg.exe are needed on the PATH for -WhisperModel' }
    $conf += "whisper_model = $WhisperModel", "whisper_cmd = $whisper", "ffmpeg_cmd = $ffmpeg"
} elseif ($SpeechApi) {
    $conf += "whisper_api_url = $SpeechApi"
    if ($env:MEMCP_SPEECH_API_KEY) { $conf += "whisper_api_key = $($env:MEMCP_SPEECH_API_KEY)" }
}
$confFile = Join-Path $Bridge 'bridge.conf'
Set-Content -Encoding utf8 -Path $confFile -Value $conf
Make-PrivateFile $confFile

Step '5/5  Run it'
Register-MemcpTask 'MemCP bridge' $MemcpBridge @('gateway', '--config', $confFile, '--listen', "127.0.0.1:$BridgePort") (Join-Path $Logs 'bridge.log')
if ($WhisperModel -or $SpeechApi) {
    Register-MemcpTask 'MemCP transcribe' $MemcpBridge @('transcribe', '--config', $confFile) (Join-Path $Logs 'transcribe.log') 5
}
if ($NoTasks) {
    if ($nodeProcess) { Stop-Process -Id $nodeProcess.Id -Force -ErrorAction SilentlyContinue }
    foreach ($t in $Tasks) { Disable-ScheduledTask -TaskPath $TaskPath -TaskName $t -ErrorAction SilentlyContinue | Out-Null }
    Say 'nothing left running (-NoTasks); the tasks are made but off: Enable-ScheduledTask -TaskPath \MemCP\ ...'
} else {
    Start-ScheduledTask -TaskPath $TaskPath -TaskName 'MemCP bridge'
    if ($WhisperModel -or $SpeechApi) { Start-ScheduledTask -TaskPath $TaskPath -TaskName 'MemCP transcribe' }
    Say "the node answers on $NodeUrl, the bridge on 127.0.0.1:${BridgePort}; they start again at logon."
}

Write-Host @"

The page of your memory, on this computer only:
  $NodeUrl/admin
Its console key (shown once; it opens the page, never the memory):
  $console

An assistant that runs a command reaches the memory through the bridge, knowing neither its address nor its key:
  "$MemcpBridge" connect 127.0.0.1:$BridgePort
For Claude Code:
  claude mcp add memcp -- "$MemcpBridge" connect 127.0.0.1:$BridgePort

Logs:     $Logs
Update:   run this installer again; your memory is left alone.
Remove:   install.ps1 -Uninstall   (the memory is kept; add -Purge to delete it)
"@
