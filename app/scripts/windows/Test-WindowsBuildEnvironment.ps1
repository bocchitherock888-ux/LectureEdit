#requires -Version 7.0
param([switch]$InstallToolchain)
. (Join-Path $PSScriptRoot 'Windows-Common.ps1')
Test-PowerShellFiles
Initialize-WindowsBuildEnvironment -InstallToolchain:$InstallToolchain
