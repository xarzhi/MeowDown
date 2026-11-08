# MeowDown 打包脚本
#
# 用法（在项目根目录）：
#   powershell -ExecutionPolicy Bypass -File .\package.ps1 -Version 0.2.0
#
# 只要安装包（NSIS setup.exe）：
#   powershell -ExecutionPolicy Bypass -File .\package.ps1 -Version 0.2.0 -Setup
#
# 只打包当前版本（不改 Cargo.toml）：
#   powershell -ExecutionPolicy Bypass -File .\package.ps1
#
# 打包并把它设为 .md 的默认应用（会写当前用户注册表）：
#   powershell -ExecutionPolicy Bypass -File .\package.ps1 -Version 0.2.0 -Register
#
# 产物：dist\MeowDown-v<版本>-win64\、dist\MeowDown-v<版本>-win64.zip，
#       加 -Setup 时还有 dist\MeowDown-v<版本>-setup.exe

param(
    # 语义化版本号，例如 0.2.0 或 0.2.0-beta.1。给了就写回 Cargo.toml，
    # 同时决定产物目录名；不给就用 Cargo.toml 里现有的版本。
    [string]$Version = "",

    # 跳过 cargo build，直接拿现成的 target\release\meowdown.exe 打包。
    [switch]$SkipBuild,

    # 打包完成后顺手把这份 exe 注册为 .md 的打开方式。
    [switch]$Register,

    # 额外用 NSIS 出一个 setup.exe 安装包。
    [switch]$Setup,

    # makensis.exe 的路径。默认从 PATH 和 NSIS 的常见安装位置找。
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

# 找一个能用的 makensis：显式给的优先，其次是 PATH，最后是 NSIS 装完会去的
# 几个地方（含项目自带的 tools\nsis，方便不带 NSIS 的机器直接出安装包）。
function Find-Makensis([string]$Explicit) {
    if ($Explicit -ne "") {
        if (Test-Path $Explicit) { return (Resolve-Path $Explicit).Path }
        throw "找不到 makensis：$Explicit"
    }
    $onPath = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }

    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"),
        (Join-Path $env:ProgramFiles "NSIS\makensis.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\NSIS\makensis.exe"),
        (Join-Path $root "tools\nsis\makensis.exe")
    )
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return $candidate }
    }
    throw "没找到 makensis.exe。装一个 NSIS（winget install NSIS.NSIS），或者用 -Makensis 指定路径。"
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

# ---------------------------------------------------------------- 组包

$name = "MeowDown-v$version-win64"
$stage = Join-Path $root (Join-Path $OutDir $name)
$zip = Join-Path $root (Join-Path $OutDir "$name.zip")

if (Test-Path $stage) { [System.IO.Directory]::Delete($stage, $true) }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

# 图标和字体都编进了 exe，真正要随包的只有程序本身和字体许可证。
Copy-Item $exe (Join-Path $stage "MeowDown.exe")
Copy-Item (Join-Path $root "assets\fonts\OFL.txt") (Join-Path $stage "OFL.txt")
Copy-Item (Join-Path $root "README.md") (Join-Path $stage "README.md")

Write-Step ("单文件 {0:N1} MB" -f ((Get-Item (Join-Path $stage "MeowDown.exe")).Length / 1MB))

# 用 .NET 的删除而不是 Remove-Item：这样在有回收站钩子的环境里也能直接覆盖旧包。
if (Test-Path $zip) { [System.IO.File]::Delete($zip) }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal
Write-Step ("压缩包 {0:N1} MB" -f ((Get-Item $zip).Length / 1MB))

# ---------------------------------------------------------------- 安装包

$setupExe = $null
if ($Setup) {
    Write-Step "用 NSIS 打安装包"
    $makensis = Find-Makensis $Makensis
    Write-Step "makensis：$makensis"

    # VIProductVersion 只认四段纯数字，0.2.0-beta.1 得先摘成 0.2.0。
    $numeric = ($version -split '[-+]')[0]
    $parts = @($numeric -split '\.')
    while ($parts.Count -lt 4) { $parts += "0" }
    $quad = ($parts[0..3] -join '.')

    $setupExe = Join-Path $root (Join-Path $OutDir "MeowDown-v$version-setup.exe")
    if (Test-Path $setupExe) { [System.IO.File]::Delete($setupExe) }

    & $makensis /V2 `
        "/DVERSION=$version" `
        "/DVERSION_QUAD=$quad" `
        "/DOUTFILE=$setupExe" `
        "/DSRCDIR=$root" `
        (Join-Path $root "installer.nsi")
    if ($LASTEXITCODE -ne 0) { throw "makensis 编译失败（退出码 $LASTEXITCODE）" }

    Write-Step ("安装包 {0:N1} MB" -f ((Get-Item $setupExe).Length / 1MB))
}

# ---------------------------------------------------------------- 关联

if ($Register) {
    Write-Step "注册 .md 关联"
    & (Join-Path $stage "MeowDown.exe") --register
}

Write-Host ""
Write-Host "完成：" -ForegroundColor Green
Write-Host "  目录  $stage"
Write-Host "  压缩包 $zip"
if ($setupExe) { Write-Host "  安装包 $setupExe" }
if ($Register) {
    Write-Host ""
    Write-Host "提示：Windows 10/11 的默认程序由「设置 → 应用 → 默认应用」里的 UserChoice 决定，" -ForegroundColor Yellow
    Write-Host "      程序自己写不进去。要彻底生效，打开 MeowDown 的「设置 → 系统集成 → 去系统设置」选一次即可。" -ForegroundColor Yellow
}
