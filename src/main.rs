#![windows_subsystem = "windows"]

//! ImeBind (Rust) —— 按前台程序自动切换 Windows 输入法
//!
//! 编译为 windows 子系统：双击运行**不会**弹出黑色控制台窗口（和 C# 版的 /target:winexe 同理）。
//! 命令行模式的输出靠启动时 AttachConsole 到父控制台 + WriteConsoleW（见 log::say）。
//!
//! 用法：
//!   imebind                                        常驻（带托盘图标），按 rules.txt 自动切换
//!   imebind --no-tray                              常驻但不建托盘图标（无界面模式）
//!   imebind --help                                 打印用法
//!   imebind --status                               打印前台程序与当前输入法（--probe 同义）
//!   imebind --list                                 列出本机键盘类输入法及其 TIP
//!   imebind --activate <TIP|HKL> [session|process] 手动激活指定输入法
//!   imebind --dump-rules                           打印解析出的规则（自检用）
//!   imebind --menu-dump                            打印托盘菜单内容（自检用，不用点鼠标）
//!
//! 托盘菜单：状态 / 前台程序 / 规则一览 / 暂停·恢复 / 重新加载 rules.txt /
//!           编辑 rules.txt / 查看日志 / 打开程序所在文件夹 / 退出（退出时会还原输入法）
mod log;
mod rules;
mod tray;
mod tsf;

use log::{say, Log};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;
use windows::core::Result;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// 致命错误：写日志 + 弹消息框。
///
/// 本程序是 GUI 子系统，双击运行时**没有控制台**，如果只写日志，用户看到的就是
/// "双击了但什么都没发生"——所以这类情况必须弹窗。
fn fatal(log: &Log, msg: &str) {
    log.write(msg);
    let text = tray::to_utf16(msg);
    let title = tray::to_utf16("ImeBind");
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            None,
            windows::core::PCWSTR(text.as_ptr()),
            windows::core::PCWSTR(title.as_ptr()),
            windows::Win32::UI::WindowsAndMessaging::MB_OK
                | windows::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
        );
    }
}

fn main() -> Result<()> {
    unsafe {
        // 先附加到调用者的控制台（双击运行时没有父控制台，失败也无所谓），
        // 这样从终端跑 --list/--status 时 log::say 的输出才看得见
        let _ = windows::Win32::System::Console::AttachConsole(
            windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
        // DPI 感知必须在创建任何窗口之前设置：否则 GetSystemMetrics(SM_CXSMICON) 恒为 16，
        // 托盘图标在高缩放下会被系统拉伸变虚
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let logger = Log::new(dir.join("imebind.log"));
    let rules_path = dir.join("rules.txt");

    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("");

    // --help 不需要 COM/TSF：放在 TSF 初始化之前，TSF 环境坏了它也还能打印用法
    if mode == "--help" || mode == "-h" || mode == "/?" {
        say("ImeBind —— 按前台程序自动切换 Windows 输入法");
        say("");
        say("  imebind                                    常驻（带托盘图标），按 rules.txt 自动切换");
        say("  imebind --no-tray                          常驻但不建托盘图标（无界面模式）");
        say(
            "  imebind --status                           打印前台程序与当前输入法（--probe 同义）",
        );
        say("  imebind --list                             列出本机键盘类输入法及其 TIP");
        say("  imebind --activate <TIP|HKL> [session|process] 手动激活指定输入法");
        say("  imebind --dump-rules                       打印解析出的规则（自检用）");
        say("  imebind --menu-dump                        打印托盘菜单内容（自检用）");
        say("");
        say("rules.txt 每行一条规则：程序名 = 输入法TIP（程序名不区分大小写，# 开头为注释）");
        say(&format!("规则文件: {}", rules_path.display()));
        return Ok(());
    }

    let tsf = match tsf::Tsf::new() {
        Ok(t) => t,
        Err(e) => {
            fatal(&logger, &format!("初始化 TSF 失败：{e:?}"));
            return Ok(());
        }
    };

    match mode {
        "--list" => {
            let active = tsf.active_tip();
            for p in tsf.list()? {
                let mark = if active.as_deref() == Some(p.tip().as_str()) {
                    "   ← 当前"
                } else {
                    ""
                };
                say(&format!("  {}   {}{}", p.tip(), p.name, mark));
            }
            return Ok(());
        }
        "--status" | "--probe" => {
            say(&format!("  前台程序: {}", tray::foreground_exe()));
            say(&format!(
                "  当前输入法(本进程视角): {}",
                tsf.active_tip().unwrap_or_default()
            ));
            say(&format!("  规则文件: {}", rules_path.display()));
            say(&format!("  日志文件: {}", logger.path().display()));
            return Ok(());
        }
        "--activate" => {
            let Some(tip) = args.get(2) else {
                say("用法: imebind --activate <TIP|HKL> [session|process]");
                return Ok(());
            };
            let mut flags = tsf::TF_IPPMF_FORPROCESS | tsf::TF_IPPMF_FORSESSION;
            match args.get(3).map(String::as_str) {
                Some("session") => flags = tsf::TF_IPPMF_FORSESSION,
                Some("process") => flags = tsf::TF_IPPMF_FORPROCESS,
                _ => {}
            }
            // 两种形式都收：TIP（TSF 输入法）和 HKL（键盘布局，如 0409:HKL:04090409）
            let hr = if let Some((langid, hkl)) = tsf::parse_hkl_tip(tip) {
                tsf.activate_hkl(langid, hkl, flags)
            } else {
                let (langid, clsid, profile) = match tsf::parse_tip(tip) {
                    Ok(v) => v,
                    Err(e) => {
                        say(&format!("TIP 解析失败: {e}"));
                        return Ok(());
                    }
                };
                tsf.activate(langid, &clsid, &profile, flags)
            };
            sleep(Duration::from_millis(300));
            say(&format!(
                "  激活 {tip} -> {hr:?}，当前为 {}",
                tsf.active_tip().unwrap_or_default()
            ));
            return Ok(());
        }
        "--dump-rules" => {
            rules::ensure_example(&rules_path);
            // 先打标题再解析，否则解析告警会出现在标题上面（load 里边解析边写日志）
            say(&format!("规则文件: {}", rules_path.display()));
            let list = rules::load(&rules_path, &logger, &tsf);
            say(&format!("共 {} 条：", list.len()));
            for r in &list {
                say(&format!("  {}", r.display()));
            }
            return Ok(());
        }
        _ => {}
    }

    // ── 常驻模式 ──────────────────────────────────────────────────────────
    rules::ensure_example(&rules_path);
    let list = rules::load(&rules_path, &logger, &tsf);
    let no_tray = args.iter().any(|a| a == "--no-tray");
    let mut app = tray::App::new(tsf, logger, dir, list);

    // 自检：把托盘菜单构建一遍并用 Win32 API 读回内容（不用点鼠标也能验证菜单）
    if mode == "--menu-dump" {
        app.dump_menu();
        return Ok(());
    }

    // 单实例保护：两个常驻实例会互相抢输入法。放在"启动"日志之前——排第二的
    // 实例到此为止，不留假启动记录（--menu-dump 是诊断模式，不受互斥量限制）。
    // 注意 CreateMutexW 必须配合 GetLastError 判断，否则拿不到 ERROR_ALREADY_EXISTS。
    // （句柄不需要显式关闭：HANDLE 没有 Drop 实现，进程退出时由系统回收；
    //   绑定到 _mutex 只是为了让所有权明确。）
    let _mutex = unsafe {
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;
        match CreateMutexW(None, false, windows::core::w!("ImeBind_SingleInstance")) {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    // 本程序是 GUI 子系统，双击运行时没有控制台，所以这种情况必须弹窗，
                    // 否则用户看到的就是"双击了但什么都没发生"
                    app.log.write("已有 ImeBind 实例在运行，本次退出。");
                    windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                        None,
                        windows::core::w!("已有 ImeBind 实例在运行（可能是 C# 版），本次启动取消。\r\n\r\n如果想用这个版本，请先结束正在运行的那个：\r\nStop-Process -Name ImeBind -Force"),
                        windows::core::w!("ImeBind"),
                        windows::Win32::UI::WindowsAndMessaging::MB_OK
                            | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
                    );
                    // 让 mutex 句柄活着到进程结束（_mutex 绑定），这里直接退出
                    return Ok(());
                }
                Some(h)
            }
            Err(e) => {
                app.log.write(&format!("创建互斥量失败（{e:?}），继续运行"));
                None
            }
        }
    };

    if app.rules.is_empty() {
        fatal(
            &app.log,
            &format!(
                "{} 里没有有效规则，程序无法启动。\r\n\r\n\
                 请用记事本打开它，填入至少一条规则，例如：\r\n\
                 dota2.exe = 0804:{{CLSID}}{{ProfileGUID}}\r\n\r\n\
                 可用的输入法 TIP 用 `imebind.exe --list` 查看（需在终端里运行）。",
                rules_path.display()
            ),
        );
        return Ok(());
    }
    app.log.write(&format!(
        "启动：加载 {} 条规则，当前={}",
        app.rules.len(),
        app.tsf.active_tip().unwrap_or_default()
    ));
    for r in &app.rules {
        app.log.write(&format!("  规则 {}", r.display()));
    }

    if no_tray {
        app.log.write("以无界面模式运行（--no-tray）");
        loop {
            app.poll();
            sleep(Duration::from_millis(tray::POLL_MS as u64));
        }
    } else if !app.run_with_tray()? {
        // 托盘建不出来（比如没有交互式桌面）：提示一下，然后退回无界面模式继续工作
        fatal(
            &app.log,
            "托盘图标创建失败（可能没有可用的交互式桌面）。\r\n程序将继续以无界面模式在后台运行。",
        );
        loop {
            app.poll();
            sleep(Duration::from_millis(tray::POLL_MS as u64));
        }
    }
    Ok(())
}
