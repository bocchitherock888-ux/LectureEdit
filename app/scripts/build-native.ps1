#requires -Version 7.0
param(
    [ValidateSet('multi','baseline')][string]$CpuProfile = 'multi',
    [ValidateRange(1,32)][int]$Jobs = 4
)
. (Join-Path $PSScriptRoot 'windows\Windows-Common.ps1')
Initialize-WindowsBuildEnvironment -InstallToolchain
$source = Join-Path $script:AppDir '.native-build\source\llama'
$build = Join-Path $script:AppDir ".native-build\windows-x64-$CpuProfile"
$native = Join-Path $script:AppDir 'src-tauri\resources\native'
Push-Location $script:RepoDir
try {
    & (Join-Path $script:AppDir 'native\windows-loopback\build.ps1')
    if (-not (Test-Path (Join-Path $script:AppDir 'native\bin\lectureedit-loopback.exe'))) { throw 'Missing WASAPI helper output.' }
    if (-not (Test-Path (Join-Path $source '.git'))) {
        if (Test-Path $source) { throw "Expected a Git checkout: $source. Move this generated directory aside and retry." }
        New-Item -ItemType Directory -Force -Path $source | Out-Null
        Invoke-Checked git.exe @('-C',$source,'init')
        Invoke-Checked git.exe @('-C',$source,'remote','add','origin','https://github.com/ggml-org/llama.cpp.git')
    }
    $origin = (& git.exe -C $source remote get-url origin).Trim()
    if ($LASTEXITCODE -ne 0 -or $origin -ne 'https://github.com/ggml-org/llama.cpp.git') { throw "Unexpected llama.cpp origin: $origin" }
    & git.exe -C $source cat-file -e "$($script:LlamaCommit)^{commit}" 2>$null
    if ($LASTEXITCODE -ne 0) { Invoke-Checked git.exe @('-C',$source,'fetch','--depth','1','origin',$script:LlamaCommit) }
    Invoke-Checked git.exe @('-C',$source,'diff','--quiet')
    Invoke-Checked git.exe @('-C',$source,'diff','--cached','--quiet')
    Invoke-Checked git.exe @('-C',$source,'checkout','--detach',$script:LlamaCommit)
    if ((& git.exe -C $source rev-parse HEAD).Trim() -ne $script:LlamaCommit) { throw 'llama.cpp commit mismatch.' }
    $options = @('-S',$source,'-B',$build,'-G','Visual Studio 17 2022','-A','x64','-T','host=x64',
        "-DCMAKE_GENERATOR_INSTANCE=$script:VisualStudioDir",'-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL',
        '-DGGML_NATIVE=OFF','-DGGML_OPENMP=OFF','-DGGML_CUDA=OFF','-DGGML_HIP=OFF',
        '-DGGML_VULKAN=OFF','-DGGML_SYCL=OFF','-DGGML_OPENCL=OFF','-DGGML_BLAS=OFF','-DGGML_METAL=OFF',
        '-DLLAMA_OPENSSL=OFF','-DLLAMA_CURL=OFF','-DLLAMA_BUILD_UI=OFF','-DLLAMA_USE_PREBUILT_UI=OFF',
        '-DLLAMA_BUILD_SERVER=ON','-DLLAMA_BUILD_APP=OFF','-DLLAMA_BUILD_EXAMPLES=OFF',
        '-DLLAMA_BUILD_TESTS=OFF','-DLLAMA_BUILD_TOOLS=ON','-DBUILD_SHARED_LIBS=ON',
        '-DGGML_BACKEND_DIR=')
    if ($CpuProfile -eq 'multi') {
        $options += @('-DGGML_BACKEND_DL=ON','-DGGML_CPU_ALL_VARIANTS=ON')
    } else {
        $options += @('-DGGML_BACKEND_DL=OFF','-DGGML_CPU_ALL_VARIANTS=OFF',
            '-DGGML_SSE42=OFF','-DGGML_AVX=OFF','-DGGML_AVX2=OFF','-DGGML_BMI2=OFF',
            '-DGGML_FMA=OFF','-DGGML_F16C=OFF','-DGGML_AVX512=OFF','-DGGML_AVX512_VBMI=OFF',
            '-DGGML_AVX512_VNNI=OFF','-DGGML_AVX512_BF16=OFF','-DGGML_AVX_VNNI=OFF',
            '-DGGML_AMX_TILE=OFF','-DGGML_AMX_INT8=OFF','-DGGML_AMX_BF16=OFF')
    }
    Invoke-Checked cmake.exe $options
    Invoke-Checked cmake.exe @('--build',$build,'--config','Release','--target','llama-server','--parallel',"$Jobs")
    $bin = Join-Path $build 'bin\Release'
    if (-not (Test-Path (Join-Path $bin 'llama-server.exe') -PathType Leaf)) { throw "Expected server output in $bin" }
    # llama.cpp is split into several DLLs that pass C++ objects and FILE handles to one another.
    # They must share one CRT, so the build uses /MD and ships Microsoft's app-local VC++ runtime.
    $crt = Get-ChildItem -Path (Join-Path $script:VisualStudioDir 'VC\Redist\MSVC\*\x64\Microsoft.VC143.CRT') -Directory |
        Where-Object { $_.Parent.Parent.Name -match '^\d+\.\d+\.\d+$' } | Sort-Object { [version]$_.Parent.Parent.Name } -Descending | Select-Object -First 1
    if (-not $crt) { throw 'Missing Visual Studio x64 VC143 CRT redistributable directory.' }
    Get-ChildItem -LiteralPath $crt.FullName -Filter '*.dll' | Copy-Item -Destination $bin -Force
    # Only this generated resource directory is replaced. User courses/model caches are untouched.
    if (Test-Path $native) { Remove-Item -LiteralPath $native -Recurse -Force }
    $previous = $env:LECTUREEDIT_LLAMA_BIN
    try {
        $env:LECTUREEDIT_LLAMA_BIN = $bin
        Invoke-Checked node.exe @((Join-Path $script:AppDir 'scripts\package-native.mjs'))
    } finally {
        if ($null -eq $previous) { Remove-Item Env:LECTUREEDIT_LLAMA_BIN -ErrorAction SilentlyContinue } else { $env:LECTUREEDIT_LLAMA_BIN = $previous }
    }
    $auditDir = Join-Path $script:RepoDir '.windows-build'
    New-Item -ItemType Directory -Force -Path $auditDir | Out-Null
    Invoke-Checked node.exe @((Join-Path $script:AppDir 'scripts\windows\audit-windows.mjs'),
        '--native-root',$native,'--profile',$CpuProfile,'--dll-check','--smoke',
        '--out',(Join-Path $auditDir 'native-audit.json'))
} finally { Pop-Location }
