# Builds ScreenCraft for Windows and packs the installer.
#
# Needs, on a Windows 10/11 PC:
#   - Rust with the MSVC toolchain  (https://rustup.rs, and the "Desktop development
#     with C++" workload from the Visual Studio Build Tools)
#   - NSIS                          (winget install NSIS.NSIS)
#   - CMake                         (winget install Kitware.CMake) - builds libopus and whisper.cpp
#   - LLVM (libclang)               (winget install LLVM.LLVM) - lets the whisper.cpp bindings generate
#
# Usage (from the repository root, in PowerShell):
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Out custom-name.exe
# The installer is named after the version (0.4.1 -> screencraft_0_4_1.exe), like the macOS .dmg.
param([string]$Out = "")

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

# Load the Visual Studio C++ environment (INCLUDE/LIB/PATH) even when this isn't a
# "Developer PowerShell".
if (-not $env:INCLUDE) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        $vcvars = if ($vs) { Join-Path $vs "VC\Auxiliary\Build\vcvars64.bat" } else { $null }
        if ($vcvars -and (Test-Path $vcvars)) {
            cmd /c "`"$vcvars`" >nul && set" | ForEach-Object {
                if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
            }
        }
    }
}
if (-not $env:INCLUDE) { throw "Visual C++ environment not found - install the 'Desktop development with C++' workload" }

# CMake may be installed but not on this session's PATH yet.
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    $cm = Join-Path $env:ProgramFiles "CMake\bin"
    if (Test-Path (Join-Path $cm "cmake.exe")) { $env:PATH = "$cm;$env:PATH" }
    else { throw "cmake not found - install it first (winget install Kitware.CMake), then open a new PowerShell" }
}

# CMake 4 dropped support for projects that declare an old minimum version (libopus does).
$env:CMAKE_POLICY_VERSION_MINIMUM = "3.5"

# LLVM's libclang for the whisper.cpp bindings, when it isn't on PATH yet.
if (-not $env:LIBCLANG_PATH -and (Test-Path "C:\Program Files\LLVM\bin\libclang.dll")) {
    $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
}

# Caching. target\ is kept between builds (cargo only rebuilds changed crates). sccache, when
# installed, also caches every rustc and C/C++ compile (whisper.cpp, libopus), so a clean
# target\ or a dependency bump doesn't recompile everything from scratch.
$sccache = (Get-Command sccache -ErrorAction SilentlyContinue).Source
if ($sccache) {
    $env:RUSTC_WRAPPER = $sccache
    $env:CMAKE_C_COMPILER_LAUNCHER = $sccache
    $env:CMAKE_CXX_COMPILER_LAUNCHER = $sccache
    if (-not $env:SCCACHE_DIR) { $env:SCCACHE_DIR = Join-Path $env:LOCALAPPDATA "sccache" }
    if (-not $env:SCCACHE_CACHE_SIZE) { $env:SCCACHE_CACHE_SIZE = "20G" }
    # sccache can't cache incremental builds; release builds aren't incremental anyway
    $env:CARGO_INCREMENTAL = "0"
    Write-Host "sccache: $sccache ($env:SCCACHE_DIR)"
}
$sw = [Diagnostics.Stopwatch]::StartNew()

cargo build --release -p screencraft
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value

$makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $makensis) {
    foreach ($dir in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        $candidate = Join-Path $dir "NSIS\makensis.exe"
        if (Test-Path $candidate) { $makensis = $candidate; break }
    }
}
if (-not $makensis) { throw "makensis not found - install NSIS first (winget install NSIS.NSIS)" }

if (-not $Out) { $Out = "screencraft_$($version -replace '\.', '_').exe" }
$exe = (Resolve-Path "target\release\screencraft.exe").Path
$ico = (Resolve-Path "assets\icons\icon.ico").Path
$outPath = Join-Path (Get-Location) $Out

& $makensis "/DVERSION=$version" "/DEXE_PATH=$exe" "/DICON_PATH=$ico" "/DOUT_FILE=$outPath" "scripts\installer.nsi"
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }

Write-Host ""
Write-Host ("Build time: {0:mm\:ss}" -f $sw.Elapsed)
if ($sccache) { & $sccache --show-stats | Select-String 'Cache hits|Cache misses|Compile requests' }
Write-Host "Built $outPath"
Write-Host "SHA-256: $((Get-FileHash $outPath -Algorithm SHA256).Hash)"
