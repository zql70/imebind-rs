//! TSF（Text Services Framework）封装：读取 / 激活 / 列举输入法
//!
//! Windows 没有"按应用绑定输入法"的原生功能，而所有靠键盘布局码(HKL)的方案
//! （WM_INPUTLANGCHANGEREQUEST / LoadKeyboardLayout）都无法区分共用同一个 HKL 的两个 TSF 输入法。
//! 唯一可行的办法是调 `ITfInputProcessorProfileMgr::ActivateProfile`。
use windows::core::{Result, BSTR, GUID};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr, ITfInputProcessorProfiles,
    GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE,
};

// 这几个在 msctf.idl 里是 #define（不是枚举），windows crate 不导出，自己定义
pub const TF_PROFILETYPE_INPUTPROCESSOR: u32 = 0x0001;
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
