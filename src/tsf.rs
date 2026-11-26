//! TSF（Text Services Framework）封装：读取 / 激活 / 列举输入法
//!
//! 注意：`ITfInputProcessorProfileMgr::ActivateProfile` 只可靠地作用于 TSF 感知的程序；
//! 对 IMM32/CUAS 程序（如 WezTerm）经常返回 `Ok(())` 但目标线程实际不变（已实测）。
//! 所以键盘布局（HKL）另有 `request_hkl`：用 `WM_INPUTLANGCHANGEREQUEST` 跨进程切换，
//! 这是 WezTerm 这类程序唯一可靠的输入语言切换方式（但它区分不了共用同一 HKL 的两个 TIP）。
use windows::core::{Result, BSTR, GUID};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr, ITfInputProcessorProfiles,
    GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowThreadProcessId, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    WM_INPUTLANGCHANGEREQUEST,
};

// 这几个在 msctf.idl 里是 #define（不是枚举），windows crate 不导出，自己定义
pub const TF_PROFILETYPE_INPUTPROCESSOR: u32 = 0x0001;
pub const TF_PROFILETYPE_KEYBOARDLAYOUT: u32 = 0x0002;
pub const TF_IPPMF_FORPROCESS: u32 = 0x1000_0000;
pub const TF_IPPMF_FORSESSION: u32 = 0x2000_0000;

/// 一个输入法条目
#[derive(Clone, Debug)]
pub struct Profile {
    pub langid: u16,
    pub clsid: GUID,
    pub profile: GUID,
    pub name: String,
}

impl Profile {
    pub fn tip(&self) -> String {
        format_tip(self.langid, &self.clsid, &self.profile)
    }
}

/// TSF 的入口对象。持有两个接口实例（它们都来自同一个 coclass）
pub struct Tsf {
    mgr: ITfInputProcessorProfileMgr,
    profiles: ITfInputProcessorProfiles,
}

impl Tsf {
    /// 初始化 COM 并创建 TSF 对象。
    ///
    /// 注意：必须在需要使用 TSF 的那个线程上调用（COM 是 STA）。
    pub fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            let mgr: ITfInputProcessorProfileMgr =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
            let profiles: ITfInputProcessorProfiles =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
            Ok(Self { mgr, profiles })
        }
    }

    /// 读取调用线程当前激活的输入法
    pub fn active(&self) -> Result<TF_INPUTPROCESSORPROFILE> {
        let mut p = TF_INPUTPROCESSORPROFILE::default();
        let cat = GUID_TFCAT_TIP_KEYBOARD;
        unsafe { self.mgr.GetActiveProfile(&cat, &mut p)? };
        Ok(p)
    }

    /// 读取调用线程当前激活输入法的 TIP 字符串
    pub fn active_tip(&self) -> Option<String> {
        let p = self.active().ok()?;
        if p.dwProfileType == TF_PROFILETYPE_INPUTPROCESSOR {
            Some(format_tip(p.langid, &p.clsid, &p.guidProfile))
        } else {
            Some(format!("{:04X}:HKL:{:08X}", p.langid, p.hkl.0 as usize))
        }
    }

    /// 激活指定输入法。flags 用 TF_IPPMF_FORPROCESS | TF_IPPMF_FORSESSION
    pub fn activate(&self, langid: u16, clsid: &GUID, profile: &GUID, flags: u32) -> Result<()> {
        unsafe {
            // 先把当前语言切过去，再激活具体 profile（与 C#/C++ 版行为一致）
            let _ = self.profiles.ChangeCurrentLanguage(langid);
            self.mgr.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                langid,
                clsid,
                profile,
                HKL::default(),
                flags,
            )
        }
    }

    /// 激活键盘布局型 profile（HKL，如"美式键盘"）。
    /// KEYBOARDLAYOUT 类型下 ActivateProfile 忽略 clsid/guidProfile，用 hkl 定位。
    pub fn activate_hkl(&self, langid: u16, hkl: HKL, flags: u32) -> Result<()> {
        unsafe {
            let _ = self.profiles.ChangeCurrentLanguage(langid);
            self.mgr.ActivateProfile(
                TF_PROFILETYPE_KEYBOARDLAYOUT,
                langid,
                &GUID::zeroed(),
                &GUID::zeroed(),
                hkl,
                flags,
            )
        }
    }

    /// 取输入法的显示名（如"微软拼音"），失败返回 None
    pub fn description(&self, langid: u16, clsid: &GUID, profile: &GUID) -> Option<String> {
        unsafe {
            let b: BSTR = self
                .profiles
                .GetLanguageProfileDescription(clsid, langid, profile)
                .ok()?;
            let s = b.to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
    }

    /// 列举本机所有**键盘类**输入法（过滤掉触控、手写、语音识别等）
    pub fn list(&self) -> Result<Vec<Profile>> {
        let mut out = Vec::new();
        unsafe {
            let mut p_lang: *mut u16 = std::ptr::null_mut();
            let mut count: u32 = 0;
            self.profiles.GetLanguageList(&mut p_lang, &mut count)?;
            if p_lang.is_null() {
                return Ok(out);
            }

            for i in 0..count as isize {
                let langid = *p_lang.offset(i);
                // windows crate 把 out 参数包成了返回值：EnumProfiles(langid) -> Result<IEnumTfInputProcessorProfiles>
                let Ok(en) = self.mgr.EnumProfiles(langid) else {
                    continue;
                };

                loop {
                    // Next 收的是切片（不是 C 版本的"数量+指针+指针"）
                    let mut items = [TF_INPUTPROCESSORPROFILE::default(); 1];
                    let mut fetched: u32 = 0;
                    if en.Next(&mut items, &mut fetched).is_err() || fetched == 0 {
                        break;
                    }
                    let p = items[0];
                    if p.dwProfileType != TF_PROFILETYPE_INPUTPROCESSOR {
                        continue;
                    }
                    if p.catid != GUID_TFCAT_TIP_KEYBOARD {
                        continue;
                    }
                    let tip = format_tip(p.langid, &p.clsid, &p.guidProfile);
                    if out.iter().any(|x: &Profile| x.tip() == tip) {
                        continue; // 0x0000 语言会重复枚举
                    }
                    let name = self
                        .description(langid, &p.clsid, &p.guidProfile)
                        .unwrap_or_else(|| tip.clone());
                    out.push(Profile {
                        langid: p.langid,
                        clsid: p.clsid,
                        profile: p.guidProfile,
                        name,
                    });
                }
            }

            CoTaskMemFree(Some(p_lang as *const core::ffi::c_void));
        }
        Ok(out)
    }
}

/// 当前窗口所在线程的键盘布局（HKL）
pub fn window_hkl(hwnd: HWND) -> Option<HKL> {
    let tid = unsafe { GetWindowThreadProcessId(hwnd, None) };
    if tid == 0 {
        None
    } else {
        Some(unsafe { GetKeyboardLayout(tid) })
    }
}

/// 请求窗口切换到指定键盘布局，并读回确认。
///
/// `ActivateProfile` 对别的进程（尤其 IMM32/CUAS 程序）经常返回 Ok 但实际不变；
/// `WM_INPUTLANGCHANGEREQUEST` 由目标窗口的 DefWindowProc 处理，是跨进程切布局的可靠方式。
/// 注意它只能切到 HKL，区分不了共用同一个 HKL 的两个 TIP。
pub fn request_hkl(hwnd: HWND, hkl: HKL) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    let mut result: usize = 0;
    let r = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_INPUTLANGCHANGEREQUEST,
            WPARAM(0),
            LPARAM(hkl.0 as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            1000,
            Some(&mut result),
        )
    };
    r.0 != 0 && window_hkl(hwnd) == Some(hkl)
}

/// 一个键盘布局（HKL）条目
#[derive(Clone, Debug)]
pub struct HklInfo {
    pub langid: u16,
    pub hkl: HKL,
    pub name: String,
}

/// 枚举当前加载的键盘布局；按 HKL 去重
pub fn list_hkl() -> Vec<HklInfo> {
    let n = unsafe { GetKeyboardLayoutList(None) };
    if n <= 0 {
        return Vec::new();
    }
    let mut list = vec![HKL::default(); n as usize];
    let got = unsafe { GetKeyboardLayoutList(Some(&mut list)) };
    if got <= 0 {
        return Vec::new();
    }
    list.truncate(got as usize);
    let mut out: Vec<HklInfo> = Vec::new();
    for h in list {
        if out.iter().any(|x| x.hkl == h) {
            continue;
        }
        let raw = h.0 as usize as u32;
        let langid = (raw & 0xFFFF) as u16;
        out.push(HklInfo {
            langid,
            hkl: h,
            name: hkl_label(langid, raw),
        });
    }
    out
}

/// HKL 的展示名。标准键盘布局没有 `ImmGetDescription`，按语言/HKL 给常见名字。
pub fn hkl_label(langid: u16, hkl: u32) -> String {
    let lang = match langid {
        0x0409 => "英语(美国)",
        0x0804 => "中文(简体)",
        0x0411 => "日语",
        0x0412 => "韩语",
        _ => "",
    };
    let layout = match hkl {
        0x0409_0409 => "美式键盘",
        _ => "",
    };
    match (lang, layout) {
        ("", "") => format!("HKL 0x{hkl:08X}"),
        (l, "") => l.to_string(),
        ("", k) => k.to_string(),
        (l, k) => format!("{l} {k}"),
    }
}

/// 格式化成和 PowerShell `Get-WinUserLanguageList` 里 InputMethodTips 一样的写法
pub fn format_tip(langid: u16, clsid: &GUID, profile: &GUID) -> String {
    format!(
        "{langid:04X}:{{{}}}{{{}}}",
        guid_str(clsid),
        guid_str(profile)
    )
}

/// GUID 转 "XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"（大写）
fn guid_str(g: &GUID) -> String {
    let d4 = &g.data4;
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        g.data1, g.data2, g.data3, d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]
    )
}

/// 解析 "0804:{CLSID}{ProfileGUID}" 形式的 TIP 字符串
pub fn parse_tip(tip: &str) -> std::result::Result<(u16, GUID, GUID), String> {
    let colon = tip.find(':').ok_or("缺少冒号")?;
    if colon != 4 {
        return Err("语言 ID 应为 4 位十六进制".into());
    }
    let langid = u16::from_str_radix(&tip[..4], 16).map_err(|e| e.to_string())?;
    let open1 = tip.find('{').ok_or("缺少第一个 {")?;
    let close1 = tip.find('}').ok_or("缺少第一个 }")?;
    let open2 = tip[close1..]
        .find('{')
        .map(|i| i + close1)
        .ok_or("缺少第二个 {")?;
    let close2 = tip[open2..]
        .find('}')
        .map(|i| i + open2)
        .ok_or("缺少第二个 }")?;
    let clsid = parse_guid(&tip[open1 + 1..close1])?;
    let profile = parse_guid(&tip[open2 + 1..close2])?;
    Ok((langid, clsid, profile))
}

/// 解析 "0804:HKL:08040804" 形式的键盘布局串（active_tip 对键盘布局型 profile 的输出）。
/// 不是该形式返回 None——调用方再退回 parse_tip。
pub fn parse_hkl_tip(tip: &str) -> Option<(u16, HKL)> {
    let (lang, rest) = tip.split_once(':')?;
    if lang.len() != 4 {
        return None;
    }
    let langid = u16::from_str_radix(lang, 16).ok()?;
    let hkl = usize::from_str_radix(rest.strip_prefix("HKL:")?, 16).ok()?;
    Some((langid, HKL(hkl as *mut core::ffi::c_void)))
}

/// windows-core 没有提供 GUID::from_str，所以自己拆字段
pub fn parse_guid(s: &str) -> std::result::Result<GUID, String> {
    let s = s.trim_matches(|c| c == '{' || c == '}');
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return Err(format!("GUID 段数不对: {s}"));
    }
    let d1 = u32::from_str_radix(parts[0], 16).map_err(|e| e.to_string())?;
    let d2 = u16::from_str_radix(parts[1], 16).map_err(|e| e.to_string())?;
    let d3 = u16::from_str_radix(parts[2], 16).map_err(|e| e.to_string())?;
    let tail = format!("{}{}", parts[3], parts[4]);
    if tail.len() != 16 {
        return Err(format!("GUID 尾部长度不对: {tail}"));
    }
    let mut d4 = [0u8; 8];
    for i in 0..8 {
        d4[i] = u8::from_str_radix(&tail[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(GUID::from_values(d1, d2, d3, d4))
}
