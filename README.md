# imebind

按前台程序自动切换 Windows 输入法（TSF）。C# 版（`ime-per-app`）的 Rust 重写。

**功能已与 C# 版对齐**：托盘图标与右键菜单、单实例保护、退出时还原输入法都已完成，可以直接替代 C# 版使用。

## 它解决什么问题

Windows 没有"给某个程序绑定某个输入法"的原生功能，而常见的靠键盘布局码（HKL）实现的方案，**无法区分共用同一个 HKL 的两个 TSF 输入法**（例如中文(简体)下的"微软拼音"和第三方输入法，HKL 都是 `0x08040804`）。唯一可行的办法是调用 TSF 的 `ITfInputProcessorProfileMgr::ActivateProfile`，本程序就是这么一个最小实现。

## 为什么用 Rust 重写

| 指标 | C# 版 | Rust 版 |
|---|---|---|
| 私有内存 | 22 MB | **1.8 MB** |
| 工作集 | 33 MB | **11.9 MB** |
| exe 体积 | 28 KB（+30 MB 运行时） | **286 KB**（自包含，零依赖） |
| 启动耗时 | 33 ms | 13 ms |

Rust 版不依赖 .NET Framework，也不用 GUI 框架（托盘是原生 `Shell_NotifyIcon`），所以常驻占用只有一个零头。

## 环境要求

- Windows 10 / 11（x64）
- 构建需要：Rust（rustup）+ **MSVC 生成工具**（含 Windows SDK，提供 `link.exe` 和 `rc.exe`）

使用者只需要一个 `imebind.exe`，不需要装任何东西。

## 构建

```powershell
pwsh -NoProfile -File tools\build.ps1
```

脚本会执行 `cargo build --release`，然后把 `target\release\imebind.exe` 复制到仓库根目录——程序运行时按 **exe 所在目录** 找 `rules.txt` / `imebind.log` / `icon.ico`。

## 用法

```powershell
imebind                                     常驻（带托盘图标），按 rules.txt 自动切换
imebind --no-tray                           常驻但不建托盘图标（无界面模式）
imebind --help                              打印用法
imebind --status                            打印前台程序与当前输入法（--probe 同义）
imebind --list                              列出本机键盘类输入法及其 TIP
imebind --activate <TIP> [session|process]  手动激活指定输入法
imebind --dump-rules                        打印解析出的规则（自检用）
imebind --menu-dump                         打印托盘菜单内容（自检用，不用点鼠标）
```

停止：托盘菜单点「退出」，或 `Stop-Process -Name imebind -Force`。

## 托盘菜单

右键任务栏图标：

```
状态：运行中
────────────
已生效的规则
  dota2.exe → 微软拼音
────────────
暂停自动切换 / 恢复自动切换
重新加载 rules.txt
────────────
编辑 rules.txt
查看日志
打开程序所在文件夹
────────────
退出
```

双击图标弹出气泡显示当前状态。**图标默认在 Win11 的折叠区里**（点任务栏的 `^` 能看到）；想让它常显就从折叠区拖出来。

## 开机自启

```powershell
pwsh -NoProfile -File scripts\install-autostart.ps1     # 在启动文件夹建快捷方式
pwsh -NoProfile -File scripts\uninstall-autostart.ps1   # 撤销（并结束正在运行的实例）
```

## rules.txt

每行一条规则，`#` 开头为注释，程序名不区分大小写（只匹配 exe 文件名，不写路径）：

```
dota2.exe = 0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}
```

TIP 字符串用 `imebind --list` 拿，格式是 `语言ID:{TIP的CLSID}{Profile的GUID}` —— 和 PowerShell 里 `Get-WinUserLanguageList` 输出的 `InputMethodTips` 是同一种写法，可以直接互换。

文件不存在时会自动生成一份带注释的模板；改动后在托盘菜单点一下「重新加载 rules.txt」即可生效（没有做文件监听）。

## 目录结构

```
imebind.exe              部署出来的可执行文件（已 gitignore）
icon.ico                 图标：既编进 exe 资源段，也被托盘运行时读取
icon.rc                  资源定义：图标 + 版本信息（给 rc.exe 用，见 build.rs）
build.rs                 构建脚本：把 icon.rc 嵌进 exe 资源段
LICENSE                  MIT
rules.txt                规则配置，首次运行会自动生成示例（已 gitignore）
imebind.log              运行日志（已 gitignore）
src/main.rs              入口：环境初始化、命令行分派、启动流程
src/tray.rs              托盘图标 + 右键菜单 + 消息循环 + 前台轮询（App 状态机）
src/tsf.rs               TSF 封装：读取 / 激活 / 列举输入法
src/rules.rs             rules.txt 解析
src/log.rs               极简日志 + 控制台输出
tools/build.ps1          构建 + 部署
scripts/install-autostart.ps1     开机自启：在启动文件夹建快捷方式
scripts/uninstall-autostart.ps1   撤销自启并结束正在运行的实例
```

程序编译为 **GUI 子系统**（双击不弹黑框）；命令行模式的输出通过启动时附加父控制台 + `WriteConsoleW` 实现。

## 已知限制

- 按**进程名**匹配，不支持通配符、窗口标题、窗口类名
- 用 250ms 轮询而非 `SetWinEventHook` 事件钩子
- 商店/MSIX 应用的窗口属于 `ApplicationFrameHost.exe`，规则里写 `charmap.exe` 这类名字不会命中
- 被任务管理器强杀时不会还原输入法（从托盘菜单「退出」会还原）
- 未做代码签名，SmartScreen 首次运行会提示"未知发布者"

## 和 C# 版的关系

两个版本**用同一个单实例互斥量名**（`ImeBind_SingleInstance`），所以**不能同时运行**——这是故意的：两边盯同一条规则时会互相抢输入法。迁移时先停掉 C# 版：

```powershell
Stop-Process -Name ImeBind -Force
pwsh -NoProfile -File ..\ime-per-app\scripts\uninstall-autostart.ps1   # 如果之前设过自启
.\imebind.exe
```

## 许可

[MIT](LICENSE)
