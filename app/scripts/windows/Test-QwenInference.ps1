#requires -Version 7.0
# Loads the packaged Qwen3-ASR model with the packaged llama-server exactly as the app does,
# then transcribes a synthetic English sentence. A --version smoke test cannot catch load-time crashes.
param(
    [Parameter(Mandatory)][string]$NativeRoot,
    [Parameter(Mandatory)][string]$ModelDir,
    [string]$Report,
    [ValidateRange(1,50)][int]$Iterations = 1
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = 'https://huggingface.co/ggml-org/Qwen3-ASR-0.6B-GGUF/resolve/928ab958557df9aa2ef1c93e0e83c7ad0933fae2'
$files = [ordered]@{
    'Qwen3-ASR-0.6B-Q8_0.gguf' = 'bca259818b50ca7c4c05e9bdb35a5dc04fa039653a6d6f3f0f331f96f6aa1971'
    'mmproj-Qwen3-ASR-0.6B-Q8_0.gguf' = '41a342b5e4c514e968cb756de6cd1b7be39eff43c44c57a2ef5fc6522e36603d'
}
New-Item -ItemType Directory -Force -Path $ModelDir | Out-Null
foreach ($name in $files.Keys) {
    $path = Join-Path $ModelDir $name
    if (-not (Test-Path -LiteralPath $path) -or (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() -ne $files[$name]) {
        & curl.exe -sSfL --retry 3 -o $path "$repo/$name"
        if ($LASTEXITCODE -ne 0) { throw "Model download failed: $name" }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() -ne $files[$name]) { throw "Model hash mismatch: $name" }
    }
}

# Non-ASCII and a space, like C:\Users\张伟\AppData — a common Chinese Windows profile path.
$work = Join-Path ([IO.Path]::GetTempPath()) "随堂 课堂测试-$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
$wav = Join-Path $work 'speech.wav'
$phrase = 'The opportunity cost of studying economics is the time you could have spent sleeping.'
# System.Speech lives in Windows PowerShell 5.1, not PowerShell 7.
$speak = "Add-Type -AssemblyName System.Speech; `$s = New-Object System.Speech.Synthesis.SpeechSynthesizer; " +
    "`$f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo 16000,([System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen),([System.Speech.AudioFormat.AudioChannel]::Mono); " +
    "`$s.SetOutputToWaveFile('$wav', `$f); `$s.Speak('$phrase'); `$s.Dispose()"
& powershell.exe -NoProfile -NonInteractive -Command $speak
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $wav)) { throw 'Speech synthesis failed.' }

$server = Join-Path $NativeRoot 'qwen\llama-server.exe'
$key = [guid]::NewGuid().ToString('N')
$env:LLAMA_API_KEY = $key
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0); $listener.Start()
$port = $listener.LocalEndpoint.Port; $listener.Stop()
$stderr = Join-Path $work 'server.err.log'; $stdout = Join-Path $work 'server.out.log'
$arguments = @('-m',(Join-Path $ModelDir 'Qwen3-ASR-0.6B-Q8_0.gguf'),'--mmproj',(Join-Path $ModelDir 'mmproj-Qwen3-ASR-0.6B-Q8_0.gguf'),
    '--host','127.0.0.1','--port',"$port",'--no-webui','--jinja','-ngl','0','-c','4096','-np','1','--cache-ram','0')
$process = Start-Process -FilePath $server -ArgumentList ($arguments | ForEach-Object { '"{0}"' -f $_ }) -WorkingDirectory $work `
    -RedirectStandardError $stderr -RedirectStandardOutput $stdout -PassThru -NoNewWindow
$headers = @{ Authorization = "Bearer $key" }
$result = [ordered]@{ checked_utc = [DateTime]::UtcNow.ToString('o'); expected = $phrase }
try {
    $healthy = $false
    $deadline = [DateTime]::UtcNow.AddSeconds(180)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($process.HasExited) { break }
        try {
            if ((Invoke-WebRequest -Uri "http://127.0.0.1:$port/health" -Headers $headers -TimeoutSec 2).StatusCode -eq 200) { $healthy = $true; break }
        } catch {}
        Start-Sleep -Milliseconds 500
    }
    if (-not $healthy) {
        $code = if ($process.HasExited) { '0x{0:X8}' -f $process.ExitCode } else { 'still running' }
        Write-Host '--- llama-server stderr ---'; Get-Content -LiteralPath $stderr -Tail 60 | Write-Host
        throw "llama-server did not become healthy (exit $code)."
    }
    $body = @{
        messages = @(@{ role = 'user'; content = @(@{ type = 'input_audio'; input_audio = @{ data = [Convert]::ToBase64String([IO.File]::ReadAllBytes($wav)); format = 'wav' } }) })
        stream = $false; temperature = 0; max_tokens = 256
    } | ConvertTo-Json -Depth 10
    $timings = @()
    for ($i = 0; $i -lt $Iterations; $i++) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $response = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$port/v1/chat/completions" -Headers $headers -ContentType 'application/json' -Body $body -TimeoutSec 300
        $timings += $clock.Elapsed.TotalSeconds
    }
    $audioSeconds = ((Get-Item -LiteralPath $wav).Length - 44) / 32000
    $result.audio_seconds = [math]::Round($audioSeconds, 2)
    $result.request_seconds = $timings | ForEach-Object { [math]::Round($_, 3) }
    Write-Host ('Audio {0:N2}s; request seconds: {1}' -f $audioSeconds, (($timings | ForEach-Object { $_.ToString('N3') }) -join ', '))
    $text = [string]$response.choices[0].message.content
    Write-Host "Transcript: $text"
    $result.transcript = $text
    foreach ($word in @('opportunity','cost','economics')) {
        if ($text -notmatch $word) { throw "Transcript is missing '$word'." }
    }
    $result.status = 'passed'
} finally {
    if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force; $process.WaitForExit() }
    if ($Report) { $result | ConvertTo-Json | Set-Content -LiteralPath $Report -Encoding utf8 }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host 'Qwen inference smoke test passed.'
