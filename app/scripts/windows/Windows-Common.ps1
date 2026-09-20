#requires -Version 7.0
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$script:AppDir = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$script:RepoDir = [IO.Path]::GetFullPath((Join-Path $script:AppDir '..'))
$script:WindowsTarget = 'x86_64-pc-windows-msvc'
$script:WindowsToolchain = '1.92.0-x86_64-pc-windows-msvc'
$script:LlamaCommit = '391fac16460f15233a7740550d858ac96df3419d'

function Invoke-Checked {
    param([Parameter(Mandatory)][string]$File, [string[]]$Arguments = @())
    Write-Host ('> {0} {1}' -f $File, ($Arguments -join ' '))
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Command failed ($LASTEXITCODE): $File $($Arguments -join ' ')" }
}

function Initialize-WindowsBuildEnvironment {
    param([switch]$InstallToolchain)
    if (-not $IsWindows) { throw 'Use Windows x64 or the supplied GitHub Windows workflow. This script does not cross-compile on macOS/Linux.' }
    if ([Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString() -ne 'X64' -or
        [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -ne 'X64') {
        throw 'Open native x64 PowerShell 7 on an Intel/AMD x64 Windows machine. ARM64 hosts are outside this build recipe.'
    }
    foreach ($name in @('git.exe','node.exe','npm.cmd','cmake.exe','rustup.exe','cargo.exe','python.exe')) {
        if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
            throw "Missing $name. Follow WINDOWS_BUILD.md, section B, then reopen PowerShell 7."
        }
    }
    $nodeVersion = [version]((& node.exe -p 'process.versions.node').Trim())
    if ($LASTEXITCODE -ne 0) { throw 'Node.js version check failed.' }
    if (($nodeVersion.Major -ne 22 -and $nodeVersion.Major -ne 24) -or $nodeVersion -lt [version]'22.12.0') {
        throw "Expected Node 24.x x64 (or 22.12+ within 22.x); found $nodeVersion. Keep the lockfile unchanged."
    }
    if ((& node.exe -p 'process.arch').Trim() -ne 'x64') { throw 'Install the x64 Node.js distribution.' }
    Invoke-Checked python.exe @('-c','import sys; assert sys.version_info >= (3,11)') | Out-Host
    foreach ($name in @('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS','CARGO_TARGET_DIR','CMAKE_GENERATOR','CMAKE_TOOLCHAIN_FILE','CARGO_BUILD_TARGET')) {
        if ([Environment]::GetEnvironmentVariable($name, 'Process')) {
            throw "Environment override $name is set. Use a clean PowerShell session; this recipe controls the target, CRT and output paths."
        }
    }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) { throw 'Visual Studio 2022 Build Tools missing. See WINDOWS_BUILD.md.' }
    $vs = & $vswhere -latest -products '*' -version '[17.0,18.0)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vs) { throw 'Install Visual Studio 2022 Desktop development with C++ (MSVC v143 and Windows SDK).' }
    $script:VisualStudioDir = ([string]($vs | Select-Object -First 1)).Trim()
    $devcmd = Join-Path $script:VisualStudioDir 'Common7\Tools\VsDevCmd.bat'
    $commandLine = 'call "{0}" -no_logo -arch=x64 -host_arch=x64 >nul && set' -f $devcmd
    $environment = & $env:ComSpec /d /s /c $commandLine
    if ($LASTEXITCODE -ne 0) { throw 'VS developer environment initialisation failed.' }
    foreach ($line in $environment) {
        if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($Matches[1],$Matches[2],'Process') }
    }
    foreach ($name in @('cl.exe','link.exe','rc.exe','dumpbin.exe')) {
        if (-not (Get-Command $name -ErrorAction SilentlyContinue)) { throw "VS environment missing $name. Install the Windows SDK and MSVC x64 tools." }
    }
    # Pin the toolchain only for this process and its children. Never change rustup's global default.
    $env:RUSTUP_TOOLCHAIN = $script:WindowsToolchain
    $installed = & rustup.exe toolchain list
    if ($LASTEXITCODE -ne 0) { throw 'rustup toolchain list failed.' }
    if (-not ($installed | Where-Object { $_ -like "$($script:WindowsToolchain)*" })) {
        if (-not $InstallToolchain) { throw "Run: rustup toolchain install $script:WindowsToolchain --profile minimal" }
        Invoke-Checked rustup.exe @('toolchain','install',$script:WindowsToolchain,'--profile','minimal') | Out-Host
    }
    Invoke-Checked rustup.exe @('target','add',$script:WindowsTarget,'--toolchain',$script:WindowsToolchain) | Out-Host
    Write-Host "Preflight OK: Windows x64, Node $nodeVersion, Rust $script:WindowsToolchain, VS2022."
}

function Test-PowerShellFiles {
    $files = @(Get-ChildItem -LiteralPath (Join-Path $script:AppDir 'scripts') -Recurse -Filter '*.ps1') +
             @(Get-ChildItem -LiteralPath (Join-Path $script:AppDir 'native') -Recurse -Filter '*.ps1')
    foreach ($file in $files) {
        $tokens = $null; $errors = $null
        [void][Management.Automation.Language.Parser]::ParseFile($file.FullName,[ref]$tokens,[ref]$errors)
        if ($errors.Count) { throw "PowerShell parser error in $($file.FullName): $($errors | Out-String)" }
    }
    Write-Host "PowerShell syntax OK: $($files.Count) files."
}
