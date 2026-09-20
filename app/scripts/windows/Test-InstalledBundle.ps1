#requires -Version 7.0
param([Parameter(Mandatory)][string]$InstallDir,[ValidateSet('multi','baseline')][string]$CpuProfile='multi')
. (Join-Path $PSScriptRoot 'Windows-Common.ps1')
if (-not $IsWindows) { throw 'Run this on the Windows test machine.' }
$install = (Resolve-Path -LiteralPath $InstallDir).Path
# Structural check and llama --version only; no capture, credentials, model download or app launch.
Invoke-Checked node.exe @((Join-Path $PSScriptRoot 'audit-windows.mjs'),
    '--native-root',(Join-Path $install 'native'),'--app-exe',(Join-Path $install 'lectureedit.exe'),
    '--profile',$CpuProfile,'--smoke')
Write-Host 'Installed files and server start checked. Complete WINDOWS_ACCEPTANCE.md on this machine.'
