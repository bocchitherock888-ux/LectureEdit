#requires -Version 7.0
param(
    [ValidateSet('multi','baseline')][string]$CpuProfile = 'multi',
    [ValidateRange(1,32)][int]$Jobs = 4,
    [switch]$OfflineWebView2
)
. (Join-Path $PSScriptRoot 'Windows-Common.ps1')
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
$logDir = Join-Path $script:RepoDir '.windows-build\logs'
$output = Join-Path $script:RepoDir ".windows-output\$stamp-$CpuProfile"
New-Item -ItemType Directory -Force -Path $logDir,$output | Out-Null
$log = Join-Path $logDir "build-$stamp.log"
$report = [ordered]@{
    status='in_progress'; started_utc=[DateTime]::UtcNow.ToString('o'); target=$script:WindowsTarget
    cpu_profile=$CpuProfile; rust_toolchain=$script:WindowsToolchain; llama_commit=$script:LlamaCommit
    webview2_mode=$(if ($OfflineWebView2) {'offlineInstaller'} else {'downloadBootstrapper'})
    signed=$false; gates=[ordered]@{powershell='not_run'; environment='not_run'; node_tests='not_run';
        python_tests='not_run'; frontend_tests='not_run'; frontend_build='not_run'; native_build='not_run'; licences='not_run';
        rust_tests='not_run'; bundle='not_run'; installed_app='not_run'; microphone='not_run';
        system_audio='not_run'; real_model_transcription='not_run'; macos_regression='not_run'}
}
$failure = $null
Start-Transcript -Path $log | Out-Null
Push-Location $script:AppDir
try {
    Test-PowerShellFiles; $report.gates.powershell='passed'
    Initialize-WindowsBuildEnvironment -InstallToolchain; $report.gates.environment='passed'
    $report.os_version = [Environment]::OSVersion.VersionString
    $report.msvc_file_version = (Get-Item -LiteralPath (Get-Command cl.exe).Source).VersionInfo.FileVersion
    $report.source_commit = $null
    $revision = & git.exe -C $script:RepoDir rev-parse HEAD 2>$null
    if ($LASTEXITCODE -eq 0) {
        $report.source_commit = ([string]$revision).Trim()
        $dirty = & git.exe -C $script:RepoDir status --porcelain
        if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect source worktree status.' }
        $report.source_dirty = [bool]$dirty
    }
    $report.github_run_id = $env:GITHUB_RUN_ID
    $report.node = (& node.exe --version).Trim()
    $report.cargo = (& cargo.exe --version).Trim()
    $report.cmake = ((& cmake.exe --version) | Select-Object -First 1)
    $report.visual_studio = $script:VisualStudioDir
    $report.source_locks = @{}
    foreach ($file in @('app/package-lock.json','app/src-tauri/Cargo.lock','app/native/windows-loopback/Cargo.lock','spikes/domain-rs/Cargo.lock')) {
        $report.source_locks[$file] = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $script:RepoDir $file)).Hash.ToLowerInvariant()
    }
    Invoke-Checked npm.cmd @('ci')
    Invoke-Checked npm.cmd @('run','windows:preflight-test'); $report.gates.node_tests='passed'
    Push-Location $script:RepoDir
    try { Invoke-Checked python.exe @('-m','unittest','discover','-s','tests','-v') } finally { Pop-Location }
    $report.gates.python_tests='passed'
    Invoke-Checked npm.cmd @('test'); $report.gates.frontend_tests='passed'
    Invoke-Checked npm.cmd @('run','build'); $report.gates.frontend_build='passed'
    & (Join-Path $script:AppDir 'scripts\build-native.ps1') -CpuProfile $CpuProfile -Jobs $Jobs
    $report.gates.native_build='passed'
    # Tauri's Rust build can inspect bundled resources even during cargo test.
    Invoke-Checked node.exe @((Join-Path $script:AppDir 'scripts\collect-licenses.mjs'))
    $report.gates.licences='passed'
    foreach ($manifest in @('src-tauri/Cargo.toml','native/windows-loopback/Cargo.toml','../spikes/domain-rs/Cargo.toml')) {
        Invoke-Checked cargo.exe @('test','--manifest-path',$manifest,'--locked','--target',$script:WindowsTarget,'--jobs',"$Jobs")
    }
    $report.gates.rust_tests='passed'
    $release = Join-Path $script:AppDir "src-tauri\target\$script:WindowsTarget\release"
    $bundle = Join-Path $release 'bundle\nsis'
    if (Test-Path $bundle) { Remove-Item -LiteralPath $bundle -Recurse -Force }
    $arguments = @('run','tauri','--','build','--target',$script:WindowsTarget,'--bundles','nsis')
    if ($OfflineWebView2) {
        $override = Join-Path $script:RepoDir '.windows-build\tauri.webview2-offline.json'
        @{bundle=@{windows=@{webviewInstallMode=@{type='offlineInstaller';silent=$true}}}} |
            ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $override -Encoding utf8
        $arguments += @('--config',$override)
    }
    $arguments += @('--','--locked','--jobs',"$Jobs")
    Invoke-Checked npm.cmd $arguments
    Invoke-Checked node.exe @((Join-Path $PSScriptRoot 'audit-windows.mjs'),
        '--native-root',(Join-Path $script:AppDir 'src-tauri\resources\native'),
        '--app-exe',(Join-Path $release 'lectureedit.exe'),'--profile',$CpuProfile,
        '--dll-check','--out',(Join-Path $output 'BINARY-AUDIT.json'))
    $installers = @(Get-ChildItem -LiteralPath $bundle -File -Filter '*.exe')
    if ($installers.Count -ne 1) { throw "Expected exactly one fresh NSIS installer in $bundle; found $($installers.Count)." }
    $version = (Get-Content -Raw (Join-Path $script:AppDir 'package.json') | ConvertFrom-Json).version
    $suffix = $(if ($OfflineWebView2) {'_webview2-offline'} else {''})
    $installerName = "LectureEdit_${version}_windows-x64_${CpuProfile}${suffix}_setup.exe"
    $installer = Join-Path $output $installerName
    Copy-Item -LiteralPath $installers[0].FullName -Destination $installer
    $report.installer = $installerName
    $report.installer_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
    $report.gates.bundle='passed'; $report.status='build_passed_runtime_pending'
    $report.installer_bytes = (Get-Item -LiteralPath $installer).Length
    Copy-Item -LiteralPath (Join-Path $script:RepoDir '.windows-build\native-audit.json') -Destination $output
    Write-Host "Build finished: $installer"
    Write-Host 'Next: install and run the manual acceptance checks in WINDOWS_ACCEPTANCE.md.'
} catch {
    $failure = $_; $report.status='failed'; $report.error=$_.Exception.Message
    Write-Host ($_ | Out-String)
} finally {
    $report.finished_utc=[DateTime]::UtcNow.ToString('o')
    $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $output 'BUILD-INFO.json') -Encoding utf8
    Pop-Location
    Stop-Transcript | Out-Null
    Copy-Item -LiteralPath $log -Destination (Join-Path $output 'build.log')
    Get-ChildItem -LiteralPath $output -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | Sort-Object Name | ForEach-Object {
        '{0}  {1}' -f (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant(),$_.Name
    } | Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt') -Encoding utf8
}
if ($failure) { throw $failure }
