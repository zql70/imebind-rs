# 在启动文件夹创建快捷方式，让 imebind 开机自启
# 用法：pwsh -NoProfile -File scripts\install-autostart.ps1
$ErrorActionPreference = 'Stop'
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $dir          # exe 在仓库根目录
$exe = Join-Path $root 'imebind.exe'
if (-not (Test-Path $exe)) { throw "找不到 $exe，先运行 tools\build.ps1" }

$startup = [Environment]::GetFolderPath('Startup')
$lnk = Join-Path $startup 'imebind.lnk'

$shell = New-Object -ComObject WScript.Shell
$sc = $shell.CreateShortcut($lnk)
$sc.TargetPath = $exe
$sc.WorkingDirectory = $root
$sc.WindowStyle = 7          # 最小化；程序本身是 GUI 子系统，不会显示窗口
$sc.Description = '按前台程序自动切换输入法（Rust 版）'
$sc.Save()

Write-Output ("已创建: " + $lnk)
Write-Output ("下次登录自动启动。立即启动： Start-Process '" + $exe + "'")

# 提醒：C# 版和 Rust 版共用单实例互斥量，不能同时自启。
# 注意 Windows 路径不区分大小写，所以要比对文件名的实际大小写，
# 否则会把刚创建的 imebind.lnk 误判成 C# 版的 ImeBind.lnk。
$other = Get-ChildItem $startup -Filter 'ImeBind.lnk' -ErrorAction SilentlyContinue |
         Where-Object { $_.Name -ceq 'ImeBind.lnk' }
if ($other) {
    Write-Output ""
    Write-Output ("注意：启动文件夹里还有 C# 版的自启项 " + $other.FullName)
    Write-Output "两个版本不能同时运行（共用单实例互斥量），建议删掉那个快捷方式，"
    Write-Output "或运行 imebind-csharp\scripts\uninstall-autostart.ps1。"
}
