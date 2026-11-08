# MeowDown 打包脚本 —— 只出安装包
#
# 用法（在项目根目录）：
#   powershell -ExecutionPolicy Bypass -File .\package.ps1 -Version 0.2.0
#
# 产物只有一个：dist\MeowDown-v<版本>-setup.exe（NSIS 安装包）。
# 要绿色版就直接 cargo build --release，不要把便携目录和 zip 混进生产包。
#
# 不带 -Version 就用 Cargo.toml 里现有的版本：
#   powershell -ExecutionPolicy Bypass -File .\package.ps1
#
# 已经有现成的 target\release\meowdown.exe，跳过编译：
#   powershell -ExecutionPolicy Bypass -File .\package.ps1 -Version 0.2.0 -SkipBuild
#
# 顺手把这份程序注册成 .md 的打开方式（会写当前用户注册表）：
#   powershell -ExecutionPolicy Bypass -File .\package.ps1 -Version 0.2.0 -Register

param(
    # 语义化版本号，例如 0.2.0 或 0.2.0-beta.1。给了就写回 Cargo.toml，
    # 同时决定安装包的文件名；不给就用 Cargo.toml 里现有的版本。
    [string]$Version = "",

    # 跳过 cargo build，直接拿现成的 target\release\meowdown.exe 打包。
    [switch]$SkipBuild,

    # 打包完成后顺手把这份 exe 注册为 .md 的打开方式。
    [switch]$Register,

    # 兼容旧命令：安装包本来就是唯一的产物，这个开关现在不做任何事。
    [switch]$Setup,

    # makensis.exe 的路径。默认从 PATH、NSIS 的常见安装位置、项目里的
    # tools\nsis 以及本机的便携副本里找。
    [string]$Makensis = "",

    # 产物目录，默认 dist（已在 .gitignore 里）。
    [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"
# 进度条会把控制台后续输出搅乱，关掉它。
$ProgressPreference = "SilentlyContinue"
$root = $PSScriptRoot
Set-Location $root

function Write-Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }

# 找一个能用的 makensis。顺序：显式指定的 → PATH → NSIS 装完会去的地方 →
# 项目自带的 tools\nsis → 本机解压过的便携副本（多一层版本目录）。
function Find-Makensis([string]$Explicit) {
    if ($Explicit -ne "") {
        if (Test-Path $Explicit) { return (Resolve-Path $Explicit).Path }
        throw "找不到 makensis：$Explicit"
    }
    $onPath = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }

    $roots = @(
        (Join-Path ${env:ProgramFiles(x86)} "NSIS"),
        (Join-Path $env:ProgramFiles "NSIS"),
        (Join-Path $env:LOCALAPPDATA "Programs\NSIS"),
        (Join-Path $root "tools"),
        (Join-Path $env:USERPROFILE ".workbuddy\binaries")
    )
    foreach ($dir in $roots) {
        if (-not $dir -or -not (Test-Path $dir)) { continue }
        $direct = Join-Path $dir "makensis.exe"
        if (Test-Path $direct) { return (Resolve-Path $direct).Path }
        # 解压版通常多包一层（nsis-3.11\makensis.exe）。
        $found = Get-ChildItem -Path $dir -Filter makensis.exe -Recurse -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($found) { return $found.FullName }
    }
    throw @"
没找到 makensis.exe，安装包编不出来。三种办法，任选一个：
  1. 装一个：winget install NSIS.NSIS
  2. 解压版放到项目里的 tools\nsis\（例如 tools\nsis\nsis-3.11\makensis.exe）
  3. 直接指定：-Makensis "C:\path\to\makensis.exe"
"@
}

# ---------------------------------------------------------------- 版本号

$cargoToml = Join-Path $root "Cargo.toml"
$toml = Get-Content $cargoToml -Raw

if ($Version -ne "") {
    if ($Version -notmatch '^\d+\.\d+\.\d+([.\-+][0-9A-Za-z.\-]+)?$') {
        throw "版本号格式不对：$Version（要像 0.2.0 或 0.2.0-beta.1）"
    }
    # 只改 [package] 里那一行，别动依赖的版本。
    $pattern = '(?m)^(version\s*=\s*")[^"]*(")'
    if ($toml -notmatch $pattern) { throw "Cargo.toml 里找不到 version 字段" }
    $toml = [regex]::Replace($toml, $pattern, "`${1}$Version`${2}", 1)
    Set-Content -Path $cargoToml -Value $toml -NoNewline -Encoding utf8
    Write-Step "版本号已写入 Cargo.toml：$Version"
}

if ($toml -notmatch '(?m)^version\s*=\s*"([^"]+)"') { throw "读不出 Cargo.toml 里的版本号" }
$version = $Matches[1]
Write-Step "本次打包版本：$version"

# ---------------------------------------------------------------- 编译

if (-not $SkipBuild) {
    Write-Step "cargo build --release"
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "编译失败" }
} else {
    Write-Step "跳过编译，使用现有产物"
}

$exe = Join-Path $root "target\release\meowdown.exe"
if (-not (Test-Path $exe)) { throw "找不到 $exe，先跑一次不带 -SkipBuild 的打包" }

# 确认编出来的二进制里就是刚写进去的版本号。release 版是 GUI 子系统
# （windows_subsystem = "windows"），它的 stdout 抓不到，所以版本号从 PE 的
# 版本资源里读——那是 build.rs 按 CARGO_PKG_VERSION 写进去的。
$reported = (Get-Item $exe).VersionInfo.ProductVersion
if ($reported -ne $version) {
    throw "二进制里的版本是 `"$reported`"，和 Cargo.toml 的 $version 对不上（多半是没重新编译）"
}
Write-Step "二进制自检通过：MeowDown $reported"

# ---------------------------------------------------------------- 安装包

$distDir = Join-Path $root $OutDir
if (-not (Test-Path $distDir)) { New-Item -ItemType Directory -Path $distDir -Force | Out-Null }

$setupExe = Join-Path $distDir "MeowDown-v$version-setup.exe"

Write-Step "找 makensis"
$makensis = Find-Makensis $Makensis
Write-Step "makensis：$makensis"

# VIProductVersion 只认四段纯数字，0.2.0-beta.1 得先摘成 0.2.0。
$numeric = ($version -split '[-+]')[0]
$parts = @($numeric -split '\.')
while ($parts.Count -lt 4) { $parts += "0" }
$quad = ($parts[0..3] -join '.')

# 用 .NET 的删除而不是 Remove-Item：这样在有回收站钩子的环境里也能直接覆盖旧包。
if (Test-Path $setupExe) { [System.IO.File]::Delete($setupExe) }

Write-Step "用 NSIS 打安装包"
& $makensis /V2 `
    "/DVERSION=$version" `
    "/DVERSION_QUAD=$quad" `
    "/DOUTFILE=$setupExe" `
    "/DSRCDIR=$root" `
    (Join-Path $root "installer.nsi")
if ($LASTEXITCODE -ne 0) { throw "makensis 编译失败（退出码 $LASTEXITCODE）" }
if (-not (Test-Path $setupExe)) { throw "makensis 说成功了，但没看到 $setupExe" }

Write-Step ("安装包 {0:N1} MB" -f ((Get-Item $setupExe).Length / 1MB))

# ---------------------------------------------------------------- 关联

if ($Register) {
    Write-Step "注册 .md 关联"
    & $exe --register
}

Write-Host ""
Write-Host "完成：" -ForegroundColor Green
Write-Host "  $setupExe"
if ($Register) {
    Write-Host ""
    Write-Host "提示：Windows 10/11 的默认程序由「设置 → 应用 → 默认应用」里的 UserChoice 决定，" -ForegroundColor Yellow
    Write-Host "      程序自己写不进去。要彻底生效，打开 MeowDown 的「设置 → 系统集成 → 去系统设置」选一次即可。" -ForegroundColor Yellow
}
