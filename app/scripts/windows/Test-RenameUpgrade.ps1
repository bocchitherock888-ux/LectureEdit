#requires -Version 7.0
# Installs the released LectureEdit 0.1.3, then the freshly built 随堂 installer, and checks that
# the old install is removed while the course data under the bundle identifier is untouched.
param([Parameter(Mandatory)][string]$Installer,[string]$LegacyTag='v0.1.3')
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Run this on Windows.' }
function Install([string]$Path) {
    $process = Start-Process -FilePath $Path -ArgumentList '/S' -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$Path exited with $($process.ExitCode)" }
}
function Assert([bool]$Condition,[string]$Message) { if (-not $Condition) { throw $Message }; Write-Host "ok: $Message" }

$uninstall = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
$legacyDir = Join-Path $env:LOCALAPPDATA 'LectureEdit'
$newDir = Join-Path $env:LOCALAPPDATA '随堂'
$download = Join-Path $env:RUNNER_TEMP 'legacy-installer'
gh release download $LegacyTag --pattern 'LectureEdit_*_windows-x64_multi_setup.exe' --dir $download --clobber
if ($LASTEXITCODE) { throw 'Could not download the previous release.' }
Install (Get-ChildItem $download -Filter '*.exe' | Select-Object -First 1).FullName
Assert (Test-Path (Join-Path $legacyDir 'lectureedit.exe')) 'previous release installed'
Assert (Test-Path "$uninstall\LectureEdit") 'previous release registered'

$data = Join-Path $env:APPDATA 'com.lectureedit.desktop'
New-Item -ItemType Directory -Force $data | Out-Null
$marker = Join-Path $data 'rename-upgrade-marker.txt'
Set-Content -LiteralPath $marker -Value 'course data'

Install (Resolve-Path -LiteralPath $Installer).Path
Assert (Test-Path (Join-Path $newDir 'lectureedit.exe')) 'renamed app installed'
Assert ((Get-ItemProperty "$uninstall\随堂").DisplayName -eq '随堂') 'renamed app registered as 随堂'
Assert (-not (Test-Path "$uninstall\LectureEdit")) 'previous uninstall entry removed'
Assert (-not (Test-Path $legacyDir)) 'previous install folder removed'
Assert ((Get-Content -LiteralPath $marker) -eq 'course data') 'course data kept'
Remove-Item -LiteralPath $marker
