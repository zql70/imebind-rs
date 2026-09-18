//! 极简日志：追加写入，超过 256 KB 自动重建
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// 向控制台输出。
///
/// 本程序编译为 **windows 子系统**（双击运行不弹黑框），代价是 `println!` 没有可用的
/// stdout 句柄。所以要先 AttachConsole 附加到调用者的控制台，再直接用 WriteConsoleW 写。
/// 双击运行时没有父控制台，这里就静默丢弃（日志文件里仍然有记录）。
pub fn say(msg: &str) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::WriteFile;
    use windows::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, WriteConsoleW, CONSOLE_MODE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let h: HANDLE = match GetStdHandle(STD_OUTPUT_HANDLE) {
            Ok(h) => h,
            Err(_) => return,
        };
        if h.is_invalid() || h.0.is_null() {
            return;
        }
        // 换行只在这里加一次，两个分支共用——之前只有重定向分支加了 \r\n，
        // 真控制台分支没加，导致在真控制台里所有输出挤成一行
        let line = format!("{msg}\r\n");
        let mut mode = CONSOLE_MODE(0);
        if GetConsoleMode(h, &mut mode).is_ok() {
            // 真控制台：写 UTF-16，中文不受代码页影响
            let wide: Vec<u16> = line.encode_utf16().collect();
            let mut written: u32 = 0;
            let _ = WriteConsoleW(h, &wide, Some(&mut written), None);
        } else {
            // 被重定向到管道/文件：按 UTF-8 写字节，否则重定向后什么都看不到
            let bytes = line.into_bytes();
            let mut written: u32 = 0;
            let _ = WriteFile(h, Some(&bytes), Some(&mut written), None);
        }
    }
}

pub struct Log {
    path: PathBuf,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn write(&self, msg: &str) {
        if let Ok(m) = fs::metadata(&self.path) {
            if m.len() > 262_144 {
                let _ = fs::remove_file(&self.path);
            }
        }
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{}  {}", timestamp(), msg);
        }
        say(msg);
    }
}

/// 不引第三方时间库，直接用 Win32 取本地时间（windows crate 里它是无参并返回结构体）
fn timestamp() -> String {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}
