# 构建 Rust 版，并把 exe 部署到仓库根目录
#
# 为什么要复制出来：程序运行时按 **exe 所在目录** 找 rules.txt / imebind.log / icon.ico，
# 而 cargo 的产物在 target/release/ 下。所以要么把 exe 复制到根目录（本脚本的做法），
# 要么把配置放进去 —— 前者才能和 C# 版的目录结构保持一致。
#
# 用 pwsh 7 运行：pwsh -NoProfile -File tools\build.ps1
$ErrorActionPreference = 'Stop'
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $dir

# 找 cargo：优先 PATH，找不到就退回 rustup 的默认安装位置。
# （rustup 改的是用户 PATH，只对之后新开的进程生效，所以老 shell 里可能没有 cargo）
# 这里不用 ?. 空条件运算符——那是 PowerShell 7 专有语法，5.1 会直接报语法错误
$cmd = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cmd) { $cmd.Source } else { $null }
if (-not $cargo) {
    $fallback = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path $fallback) { $cargo = $fallback }
    else { throw "找不到 cargo。请装 Rust（rustup）或把 %USERPROFILE%\.cargo\bin 加进 PATH。" }
}
Write-Output ("使用 cargo: " + $cargo)

Push-Location $root
try {
    Write-Output '=== cargo build --release ==='
    & $cargo build --release 2>&1 | ForEach-Object { "  $_" }
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }

    $from = Join-Path $root 'target\release\imebind.exe'
    $to = Join-Path $root 'imebind.exe'
    if (-not (Test-Path $from)) { throw "找不到构建产物: $from" }
    Copy-Item $from $to -Force

    # icon.ico 也要和 exe 在一起（托盘运行时读它）
    $icon = Join-Path $root 'icon.ico'
    if (-not (Test-Path $icon)) { Write-Output '（注意：根目录没有 icon.ico，托盘图标会退回 exe 内嵌图标）' }

    $f = Get-Item $to
    Write-Output ''
    Write-Output ("已部署: " + $f.FullName + "  " + $f.Length + " 字节")
} finally { Pop-Location }
