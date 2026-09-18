# 撤销 imebind 开机自启，并结束正在运行的 Rust 版实例
# 用法：pwsh -NoProfile -File scripts\uninstall-autostart.ps1
$ErrorActionPreference = 'Continue'
$startup = [Environment]::GetFolderPath('Startup')
$lnk = Join-Path $startup 'imebind.lnk'
if (Test-Path $lnk) { Remove-Item $lnk -Force; Write-Output ("已删除: " + $lnk) } else { Write-Output "启动项不存在" }

# 只结束 Rust 版的实例（按可执行文件路径区分，避免误杀 C# 版）
$r = @(Get-Process -Name imebind -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '*imebind-rs*' })
if ($r.Count -gt 0) {
    $r | Stop-Process -Force
    Write-Output ("已结束 " + $r.Count + " 个 Rust 版进程")
} else {
    Write-Output "没有正在运行的 Rust 版实例"
}
Write-Output "完成。imebind.exe 本体仍在原目录，未被删除。"
