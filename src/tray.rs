//! 托盘图标 + 右键菜单 + 消息循环
//!
//! 用原生 Win32（`Shell_NotifyIconW` / `CreatePopupMenu` / `TrackPopupMenu`），不依赖任何 GUI 框架——
//! 这是 Rust 版内存只有 1 MB 出头的原因（C# 版用 WinForms，要拖进 17 MB 的 System.Windows.Forms）。
use crate::log::Log;
use crate::rules::{self, Rule};
use crate::tsf::{self, Tsf};
use std::path::PathBuf;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// 托盘回调消息（WM_APP + 1）
const WM_TRAY: u32 = WM_APP + 1;
/// 轮询定时器
const TIMER_POLL: usize = 1;
/// 轮询间隔（无界面模式也用这个值，别再各写一份）
pub const POLL_MS: u32 = 250;
const REASSERT_MS: u128 = 1500;

const ID_PAUSE: u32 = 2000;
const ID_RELOAD: u32 = 2001;
const ID_EDIT: u32 = 2002;
const ID_LOG: u32 = 2003;
const ID_OPENDIR: u32 = 2004;
const ID_EXIT: u32 = 2005;

pub struct App {
    pub tsf: Tsf,
    pub log: Log,
    pub dir: PathBuf,
    pub rules_path: PathBuf,
    pub rules: Vec<Rule>,
    pub paused: bool,
    /// 切换前的输入法，离开规则程序（或退出）时还原
    pub forced: Option<String>,
    seen: String,
    /// 前台程序名缓存：(hwnd, pid) 没变就复用，省掉每 250ms 一次的 OpenProcess
    fg_hwnd: isize,
    fg_pid: u32,
    fg_exe: String,
    /// RegisterWindowMessageW("TaskbarCreated") 的消息号；Explorer 重启后靠它重建托盘图标
    wm_taskbar_created: u32,
    reassert_until: Option<std::time::Instant>,
    hwnd: HWND,
    nid: NOTIFYICONDATAW,
    tray_added: bool,
    last_tip: String,
}

impl App {
    pub fn new(tsf: Tsf, log: Log, dir: PathBuf, rules: Vec<Rule>) -> Self {
        let rules_path = dir.join("rules.txt");
        Self {
            tsf,
            log,
            dir,
            rules_path,
            rules,
            paused: false,
            forced: None,
            seen: String::new(),
            fg_hwnd: 0,
            fg_pid: 0,
            fg_exe: String::new(),
            wm_taskbar_created: 0,
            reassert_until: None,
            hwnd: HWND::default(),
            nid: NOTIFYICONDATAW::default(),
            tray_added: false,
            last_tip: String::new(),
        }
    }

    // ── 核心切换逻辑（无界面模式也用它） ─────────────────────────────────

    pub fn poll(&mut self) {
        let exe = self.foreground_exe();
        if self.paused {
            // 暂停时不做任何切换；把 seen 跟上，避免恢复后误报"前台切换"并重新应用规则。
            // 不刷托盘提示：tooltip 只由 paused 决定，暂停/恢复时 toggle_pause 自己会刷新
            self.seen = exe;
            return;
        }
        if exe != self.seen {
            self.log
                .write(&format!("前台切换: {} -> {}", self.seen, exe));
            match rules::find(&self.rules, &exe) {
                Some(r) => {
                    let cur = self.tsf.active_tip().unwrap_or_default();
                    if !cur.eq_ignore_ascii_case(&r.tip) {
                        // 只在第一次记录"用户原本的输入法"：在两个规则程序之间切换时，
                        // 上一个规则的目标输入法不是用户的原始状态，不能覆盖
                        if self.forced.is_none() {
                            self.forced = Some(cur.clone());
                        }
                        let hr = self.tsf.activate(
                            r.langid,
                            &r.clsid,
                            &r.profile,
                            tsf::TF_IPPMF_FORPROCESS | tsf::TF_IPPMF_FORSESSION,
                        );
                        self.log.write(&format!(
                            "应用规则 {}: {} -> {} ({hr:?})",
                            r.exe, cur, r.tip
                        ));
                    }
                    self.reassert_until = Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_millis(REASSERT_MS as u64),
                    );
                }
                None => self.restore_forced("离开规则程序"),
            }
            self.seen = exe;
        } else if let Some(deadline) = self.reassert_until {
            if std::time::Instant::now() < deadline {
                if let Some(r) = rules::find(&self.rules, &exe) {
                    let cur = self.tsf.active_tip().unwrap_or_default();
                    if !cur.eq_ignore_ascii_case(&r.tip) {
                        let hr = self.tsf.activate(
                            r.langid,
                            &r.clsid,
                            &r.profile,
                            tsf::TF_IPPMF_FORPROCESS | tsf::TF_IPPMF_FORSESSION,
                        );
                        self.log.write(&format!(
                            "补偿一次 {}: {} -> {} ({hr:?})",
                            r.exe, cur, r.tip
                        ));
                        self.reassert_until = None;
                    }
                }
            } else {
                self.reassert_until = None;
            }
        }
    }

    fn restore_forced(&mut self, why: &str) {
        if let Some(tip) = self.forced.take() {
            let flags = tsf::TF_IPPMF_FORPROCESS | tsf::TF_IPPMF_FORSESSION;
            // forced 有两种形式：TIP（TSF 输入法）和 HKL（"0804:HKL:08040804"，键盘布局如
            // "美式键盘"——active_tip 对布局型 profile 输出这种形式）。HKL 形式 parse_tip
            // 解析不了，之前在这里被 if let Ok 整个吞掉：不还原、不记日志。
            let hr = if let Some((langid, hkl)) = tsf::parse_hkl_tip(&tip) {
                self.tsf.activate_hkl(langid, hkl, flags)
            } else if let Ok((langid, clsid, profile)) = tsf::parse_tip(&tip) {
                self.tsf.activate(langid, &clsid, &profile, flags)
            } else {
                self.log
                    .write(&format!("{why}，还原目标无法解析，已放弃: {tip}"));
                return;
            };
            self.log.write(&format!("{why}，还原为 {tip} ({hr:?})"));
        }
    }

    /// 前台程序名，按 (hwnd, pid) 缓存：轮询每 250ms 一次，绝大多数时候前台没变，
    /// 直接复用上次结果。PID 会被系统复用，缓存键必须带上窗口句柄。
    fn foreground_exe(&mut self) -> String {
        let (hwnd, pid) = foreground_pid();
        if pid == 0 {
            self.fg_hwnd = 0;
            self.fg_pid = 0;
            self.fg_exe.clear();
            return String::new();
        }
        if pid == self.fg_pid && hwnd.0 as isize == self.fg_hwnd {
            return self.fg_exe.clone();
        }
        let name = query_image_name(pid);
        self.fg_hwnd = hwnd.0 as isize;
        self.fg_pid = pid;
        self.fg_exe = name.clone();
        name
    }

    // ── 托盘 ────────────────────────────────────────────────────────────

    /// 创建隐藏窗口 + 托盘图标，进入消息循环。
    /// 返回 `Ok(true)` = 正常跑完消息循环；`Ok(false)` = 托盘建不出来（调用方应退回无界面模式）
    pub fn run_with_tray(&mut self) -> windows::core::Result<bool> {
        unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
            let class = w!("ImeBindTrayWnd");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                self.log.write("RegisterClassW 失败，托盘不可用");
                return Ok(false);
            }
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("ImeBind"),
                WINDOW_STYLE(0), // 不可见、无任务栏按钮
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinst),
                None,
            ) {
                Ok(h) => h,
                Err(e) => {
                    self.log
                        .write(&format!("创建隐藏窗口失败：{e:?}，托盘不可用"));
                    return Ok(false);
                }
            };
            self.hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, self as *mut App as isize);

            // 托盘图标
            self.nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_TRAY,
                hIcon: load_icon(&self.dir),
                ..Default::default()
            };
            // 先算出字符串再写进 nid：否则会与 &mut self.nid 的可变借用冲突
            let tip = self.short_status();
            fill_utf16(&mut self.nid.szTip, &tip);
            self.last_tip = tip;
            if !Shell_NotifyIconW(NIM_ADD, &self.nid).as_bool() {
                self.log.write("添加托盘图标失败，托盘不可用");
                let _ = DestroyWindow(hwnd);
                return Ok(false);
            }
            self.tray_added = true;
            self.log
                .write(&format!("托盘图标已创建（{}px 帧）", small_icon_size()));

            // Explorer 重启时旧托盘图标随 shell 一起消失，新 shell 起来会广播 TaskbarCreated，
            // 记下消息号，wndproc 收到后重建图标
            self.wm_taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
            SetTimer(Some(hwnd), TIMER_POLL, POLL_MS, None);

            // 消息循环（GetMessageW 返回 -1 表示出错，所以判断 > 0）
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(true)
    }

    /// 托盘悬停提示：只显示运行状态（规则见右键菜单，前台变化见日志）
    fn short_status(&self) -> String {
        format!(
            "ImeBind {}",
            if self.paused {
                "[已暂停]"
            } else {
                "[运行中]"
            }
        )
    }

    fn update_tip(&mut self) {
        if !self.tray_added {
            return;
        }
        let s = self.short_status();
        if s == self.last_tip {
            return;
        }
        fill_utf16(&mut self.nid.szTip, &s);
        self.last_tip = s;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.nid);
        }
    }

    fn balloon(&mut self) {
        if !self.tray_added {
            return;
        }
        fill_utf16(&mut self.nid.szInfoTitle, "ImeBind");
        let info = format!("{}\r\n右键图标可打开菜单", self.short_status());
        fill_utf16(&mut self.nid.szInfo, &info);
        self.nid.dwInfoFlags = NIIF_INFO;
        self.nid.uFlags |= NIF_INFO;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.nid);
        }
        self.nid.uFlags &= !NIF_INFO;
    }

    /// Explorer 重启后托盘图标随旧 shell 一起消失；TaskbarCreated 到来时重建。
    /// nid 里的 hIcon 是本进程的 GDI 对象、szTip 是最新文本，都还有效，直接 NIM_ADD
    fn readd_tray_icon(&mut self) {
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &self.nid).as_bool() {
                self.log.write("Explorer 已重启，托盘图标已重建");
            } else {
                self.log.write("Explorer 重启后重建托盘图标失败");
            }
        }
    }

    /// 构建右键菜单（内容与 C# 版一致）。抽出来是为了让 show_menu 和 dump_menu 共用
    pub fn build_menu(&self) -> windows::core::Result<HMENU> {
        unsafe {
            let menu = CreatePopupMenu()?;
            append_gray(
                menu,
                &format!(
                    "状态：{}",
                    if self.paused {
                        "已暂停"
                    } else {
                        "运行中"
                    }
                ),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            if self.rules.is_empty() {
                append_gray(menu, "（rules.txt 里没有有效规则）");
            } else {
                append_gray(menu, "已生效的规则");
                for (i, r) in self.rules.iter().enumerate() {
                    if i >= 20 {
                        append_gray(menu, "  ...");
                        break;
                    }
                    append_gray(menu, &format!("  {}", r.display()));
                }
            }

            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            append_item(
                menu,
                ID_PAUSE,
                if self.paused {
                    "恢复自动切换"
                } else {
                    "暂停自动切换"
                },
            );
            append_item(menu, ID_RELOAD, "重新加载 rules.txt");
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            append_item(menu, ID_EDIT, "编辑 rules.txt");
            append_item(menu, ID_LOG, "查看日志");
            append_item(menu, ID_OPENDIR, "打开程序所在文件夹");
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            append_item(menu, ID_EXIT, "退出");
            Ok(menu)
        }
    }

    /// 自检：把菜单构建出来，再用 Win32 API 把每一项读回来打印。
    /// 这样不用真的点鼠标也能验证菜单内容（包括中文是否正确写进了菜单）。
    pub fn dump_menu(&self) {
        use crate::log::say;
        let Ok(menu) = self.build_menu() else {
            say("构建菜单失败");
            return;
        };
        unsafe {
            let count = GetMenuItemCount(Some(menu));
            say(&format!(
                "托盘菜单共 {count} 项（用 GetMenuStringW 从真实 HMENU 读回）："
            ));
            for i in 0..count {
                let mut buf = [0u16; 256];
                let n = GetMenuStringW(menu, i as u32, Some(&mut buf), MF_BYPOSITION);
                let text = if n > 0 {
                    String::from_utf16_lossy(&buf[..n as usize])
                } else {
                    String::new()
                };
                let id = GetMenuItemID(menu, i);
                if text.is_empty() {
                    say("  ---");
                    continue;
                }
                let kind = if id == u32::MAX {
                    "分隔"
                } else if id == 0 {
                    "只读"
                } else {
                    "可点击"
                };
                say(&format!("  [{kind}] {text}"));
            }
            let _ = DestroyMenu(menu);
        }
    }

    /// 弹出右键菜单并执行选择（用 TPM_RETURNCMD，省掉 WM_COMMAND 分发）
    fn show_menu(&mut self) {
        unsafe {
            let Ok(menu) = self.build_menu() else { return };

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // 这两句是托盘菜单的标准要求：先把自己设为前台，弹完再发一条空消息，
            // 否则菜单不会在点击别处时消失
            let _ = SetForegroundWindow(self.hwnd);
            let r = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                pt.x,
                pt.y,
                None, // nreserved，windows crate 里是 Option<i32>
                self.hwnd,
                None,
            );
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);

            // 先把路径克隆出来，避免与 self.open(&self) 的借用冲突
            let rules_path = self.rules_path.clone();
            let log_path = self.log.path().clone();
            let dir = self.dir.clone();
            match r.0 as u32 {
                0 => {} // 取消了
                ID_PAUSE => self.toggle_pause(),
                ID_RELOAD => self.reload_rules(),
                ID_EDIT => self.open(&rules_path),
                ID_LOG => self.open(&log_path),
                ID_OPENDIR => self.open(&dir),
                ID_EXIT => self.quit(),
                _ => {}
            }
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if self.paused {
            self.restore_forced("暂停时");
        }
        self.log.write(if self.paused {
            "已暂停自动切换"
        } else {
            "已恢复自动切换"
        });
        self.last_tip.clear();
        self.update_tip();
    }

    fn reload_rules(&mut self) {
        self.rules = rules::load(&self.rules_path, &self.log, &self.tsf);
        self.log
            .write(&format!("重新加载 rules.txt：{} 条规则", self.rules.len()));
        self.last_tip.clear();
        self.update_tip();
    }

    fn open(&self, path: &std::path::Path) {
        let target: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str())
            .chain(Some(0))
            .collect();
        unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(target.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            );
        }
    }

    fn quit(&mut self) {
        self.restore_forced("退出时");
        if self.tray_added {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.nid);
            }
            self.tray_added = false;
        }
        self.log.write("退出");
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// ── Win32 辅助 ──────────────────────────────────────────────────────────

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    match msg {
        WM_TIMER if wp.0 == TIMER_POLL => {
            if !app.is_null() {
                (*app).poll();
            }
            LRESULT(0)
        }
        WM_TRAY => {
            // 托盘图标的回调：lParam 低 16 位是鼠标消息
            match (lp.0 as u32 & 0xFFFF, app.is_null()) {
                (WM_RBUTTONUP | WM_CONTEXTMENU, false) => (*app).show_menu(),
                (WM_LBUTTONDBLCLK, false) => (*app).balloon(),
                _ => {}
            }
            LRESULT(0)
        }
        msg if !app.is_null() && msg != 0 && msg == (*app).wm_taskbar_created => {
            (*app).readd_tray_icon();
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// 字符串 → 以 NUL 结尾的 UTF-16（Win32 字符串参数用；fatal 的弹窗也用它）
pub(crate) fn to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 把字符串写进定长 UTF-16 缓冲区（NOTIFYICONDATAW 的 szTip/szInfo 都是定长数组）
fn fill_utf16(buf: &mut [u16], s: &str) {
    let v: Vec<u16> = s.encode_utf16().take(buf.len() - 1).collect();
    buf[..v.len()].copy_from_slice(&v);
    for x in buf[v.len()..].iter_mut() {
        *x = 0;
    }
}

fn append_gray(menu: HMENU, text: &str) {
    let t = to_utf16(text);
    unsafe {
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(t.as_ptr()));
    }
}

fn append_item(menu: HMENU, id: u32, text: &str) {
    let t = to_utf16(text);
    unsafe {
        let _ = AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(t.as_ptr()));
    }
}

fn small_icon_size() -> i32 {
    unsafe { GetSystemMetrics(SM_CXSMICON) }
}

/// 优先从 exe 同目录的 icon.ico 按系统 DPI 取对应帧；
/// 其次用 exe 自己内嵌的图标（icon.rc 里的资源 ID 是 1）；
/// 最后才退回系统通用图标。
fn load_icon(dir: &std::path::Path) -> HICON {
    let ico = dir.join("icon.ico");
    if ico.exists() {
        let p: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(ico.as_os_str())
            .chain(Some(0))
            .collect();
        let size = small_icon_size();
        if let Ok(h) = unsafe {
            LoadImageW(
                None,
                PCWSTR(p.as_ptr()),
                IMAGE_ICON,
                size,
                size,
                LR_LOADFROMFILE,
            )
        } {
            return HICON(h.0);
        }
    }
    unsafe {
        // 从自身 exe 的资源段取（embed-resource 把 icon.rc 编了进去，资源 ID = 1）。
        // 这里用的是 Win32 的 MAKEINTRESOURCE 技巧：把资源 ID 直接当成指针值传。
        // 用 without_provenance 而不是 `1 as *const u16`（后者会触发 clippy 的
        // manual_dangling_ptr）；也不能用 clippy 建议的 ptr::dangling::<u16>()，
        // 那给出的是地址 2（u16 的对齐值），会取到错误的资源 ID。
        if let Ok(hinst) = GetModuleHandleW(None) {
            if let Ok(h) = LoadIconW(Some(hinst.into()), PCWSTR(std::ptr::without_provenance(1))) {
                return h;
            }
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}

/// 取前台窗口所属进程的 exe 文件名（一次性查询，无缓存；轮询请走 App::foreground_exe）
pub fn foreground_exe() -> String {
    let (_, pid) = foreground_pid();
    if pid == 0 {
        return String::new();
    }
    query_image_name(pid)
}

/// 前台窗口句柄 + 所属进程 ID（没有前台窗口时 pid 为 0）
fn foreground_pid() -> (HWND, u32) {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        (hwnd, pid)
    }
}

/// 打开进程查询映像名，只返回文件名。
/// 先在 UTF-16 切片上定位最后一个反斜杠再转换，避免"整路径 + 文件名"两次 String 分配
fn query_image_name(pid: u32) -> String {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size);
        let _ = CloseHandle(h);
        if ok.is_err() {
            return String::new();
        }
        let full = &buf[..size as usize];
        let start = full
            .iter()
            .rposition(|&c| c == '\\' as u16)
            .map_or(0, |i| i + 1);
        String::from_utf16_lossy(&full[start..])
    }
}
