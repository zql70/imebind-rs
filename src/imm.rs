//! IMM32：跨进程读取 / 设置某个窗口的输入法转换模式（中文 / 英文）
//!
//! 微软拼音里的「中/英」不是输入法本身，而是输入法的转换模式（conversion mode）。
//! TSF 没有公开的跨进程接口能改**另一个窗口**的这个状态（`ITfThreadMgr` 是每线程的），
//! 所以这里走 IMM32：`ImmGetDefaultIMEWnd(hwnd)` 拿到该窗口所在线程的默认 IME 窗口，
//! 再发 `WM_IME_CONTROL`（`IMC_GET/SETCONVERSIONMODE`）。
//!
//! 有效性取决于程序是否暴露 IMM 输入上下文：WezTerm / Windows Terminal 实测可读写；
//! 部分纯 TSF 程序或第三方输入法会返回「没有 IME 窗口」，调用方记日志后忽略即可。
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_IME_CONTROL};

/// 转换模式位：native（中文）模式
pub const IME_CMODE_NATIVE: u32 = 0x0001;
/// 转换模式位：符号模式。微软拼音的中文状态为 `NATIVE | SYMBOL`（0x401），英文为 0
pub const IME_CMODE_SYMBOL: u32 = 0x0400;

const IMC_GETCONVERSIONMODE: usize = 0x0001;
const IMC_SETCONVERSIONMODE: usize = 0x0002;

/// 窗口线程的默认 IME 窗口；没有（不支持的 TSF 程序）时返回 None
fn ime_window(hwnd: HWND) -> Option<HWND> {
    let ime = unsafe { ImmGetDefaultIMEWnd(hwnd) };
    (!ime.0.is_null()).then_some(ime)
}

/// 读窗口当前的转换模式；窗口没有 IMM 输入上下文时返回 None
pub fn conversion_mode(hwnd: HWND) -> Option<u32> {
    let ime = ime_window(hwnd)?;
    let r = unsafe {
        SendMessageW(
            ime,
            WM_IME_CONTROL,
            Some(WPARAM(IMC_GETCONVERSIONMODE)),
            Some(LPARAM(0)),
        )
    };
    Some(r.0 as u32)
}

/// 设置窗口的转换模式。遵循 IME 约定：`WM_IME_CONTROL` 返回 0 表示成功
pub fn set_conversion_mode(hwnd: HWND, mode: u32) -> Option<()> {
    let ime = ime_window(hwnd)?;
    let r = unsafe {
        SendMessageW(
            ime,
            WM_IME_CONTROL,
            Some(WPARAM(IMC_SETCONVERSIONMODE)),
            Some(LPARAM(mode as isize)),
        )
    };
    (r.0 == 0).then_some(())
}
