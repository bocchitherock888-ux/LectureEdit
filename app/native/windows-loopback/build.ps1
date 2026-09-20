$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
$LASTEXITCODE = 0
$crate = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $crate
try {
    cargo build --release --locked --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw "Windows loopback helper compilation failed" }
    $built = Join-Path $crate "target\x86_64-pc-windows-msvc\release\lectureedit-windows-loopback.exe"
    $outputDir = Join-Path (Split-Path -Parent $crate) "bin"
    New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
    $binary = Join-Path $outputDir "lectureedit-loopback.exe"
    Copy-Item -Force $built $binary
    Write-Output $binary
} finally {
    Pop-Location
}
