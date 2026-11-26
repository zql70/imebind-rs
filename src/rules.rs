//! rules.txt 的解析与查找
//!
//! 格式（和 C# 版一致，程序名不区分大小写，`#` 开头为注释）：
//!   dota2.exe = 微软拼音
//!   wezterm-gui.exe = 微软拼音 en            ← 可选 en/zh，切中英文模式
//!   dota2.exe = 0804:{CLSID}{ProfileGUID}    ← 也可以直接写 TIP
use crate::log::Log;
use crate::tsf::{format_tip, parse_tip, Profile, Tsf};
use std::fs;
use std::path::Path;
use windows::core::GUID;

/// 规则可选的中英文模式（输入法的转换模式，不是另换一个输入法）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvMode {
    Chinese,
    English,
}

impl ConvMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chinese => "中文",
            Self::English => "英文",
        }
    }

    /// 规则尾部可写的记号：zh/cn/chinese/中文、en/english/英文
    fn parse(tok: &str) -> Option<Self> {
        match tok.to_ascii_lowercase().as_str() {
            "zh" | "cn" | "chinese" | "中文" => Some(Self::Chinese),
            "en" | "english" | "英文" => Some(Self::English),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub exe: String,
    pub langid: u16,
    pub clsid: GUID,
    pub profile: GUID,
    /// 规范化后的 TIP 字符串（大写），用于和当前输入法比较
    pub tip: String,
    /// 输入法显示名（如"微软拼音"），加载时解析一次；解析失败退回 TIP 字符串
    pub name: String,
    /// 聚焦该程序时把输入法切到中文/英文模式；None = 只切输入法，不动中英文
    pub mode: Option<ConvMode>,
}

impl Rule {
    pub fn display(&self) -> String {
        let mode = self
            .mode
            .map(|m| format!("（{}）", m.label()))
            .unwrap_or_default();
        format!("{} → {}{mode}", self.exe, self.name)
    }
}

/// 把规则右侧拆成「输入法名称 / TIP」和可选中英文模式。
///
/// 名称可能含空格（如 "Microsoft Pinyin"），所以只把**最后一个**能识别的记号当模式；
/// `中文` 这种整体只有一个词的名称不会被误拆。
fn split_mode(rhs: &str) -> (&str, Option<ConvMode>) {
    if let Some((head, last)) = rhs.rsplit_once(char::is_whitespace) {
        let head = head.trim();
        if !head.is_empty() {
            if let Some(mode) = ConvMode::parse(last) {
                return (head, Some(mode));
            }
        }
    }
    (rhs, None)
}

/// 第一次按名称解析时枚举一遍输入法，结果缓存在 `cache` 里供后续规则复用
fn ensure_profiles<'a>(
    tsf: &Tsf,
    cache: &'a mut Option<Vec<Profile>>,
) -> Result<&'a [Profile], String> {
    if cache.is_none() {
        let list = tsf.list().map_err(|e| format!("枚举输入法失败: {e:?}"))?;
        *cache = Some(list);
    }
    Ok(cache.as_deref().unwrap_or(&[]))
}

/// 按显示名匹配：先精确匹配，没有再 ASCII 不区分大小写匹配。
/// 不做子串匹配，避免「微软」误配到多个输入法。
fn find_by_name<'a>(list: &'a [Profile], name: &str) -> Vec<&'a Profile> {
    let exact: Vec<&Profile> = list.iter().filter(|p| p.name == name).collect();
    if !exact.is_empty() {
        return exact;
    }
    list.iter()
        .filter(|p| p.name.eq_ignore_ascii_case(name))
        .collect()
}

/// 把输入法显示名解析成 TIP 三元组；找不到或重名时返回可直接写进日志的错误
fn lookup_by_name(
    tsf: &Tsf,
    cache: &mut Option<Vec<Profile>>,
    name: &str,
) -> Result<(u16, GUID, GUID), String> {
    let list = ensure_profiles(tsf, cache)?;
    let hits = find_by_name(list, name);
    match hits.as_slice() {
        [p] => Ok((p.langid, p.clsid, p.profile)),
        [] => {
            let mut names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            if names.is_empty() {
                Err(format!(
                    "未找到输入法「{name}」，且没有枚举到任何键盘类输入法"
                ))
            } else {
                Err(format!(
                    "未找到输入法「{name}」；可用名称: {}",
                    names.join("、")
                ))
            }
        }
        _ => {
            let tips: Vec<String> = hits.iter().map(|p| p.tip()).collect();
            Err(format!(
                "输入法名称「{name}」匹配到 {} 个 TIP（{}），请改用 TIP",
                tips.len(),
                tips.join("、")
            ))
        }
    }
}

/// 文件不存在时写一份带注释的示例
pub fn ensure_example(path: &Path) {
    if path.exists() {
        return;
    }
    let _ = fs::write(
        path,
        "# ImeBind 规则：每行 \"程序名 = 输入法名称或TIP\"，程序名不区分大小写\r\n\
         # 输入法名称用 imebind --list 查看（要和显示名完全一致）\r\n\
         # 末尾可加 zh/en 指定中英文模式\r\n\
         # 示例：\r\n\
         # dota2.exe = 微软拼音\r\n\
         # wezterm-gui.exe = 微软拼音 en\r\n\
         # 名称有歧义或想固定到具体 TIP 时，可以写完整的 TIP：\r\n\
         # dota2.exe = 0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}\r\n",
    );
}

pub fn load(path: &Path, log: &Log, tsf: &Tsf) -> Vec<Rule> {
    let mut out = Vec::new();
    // 名称解析要枚举输入法；第一次用到时查一遍，后续规则复用
    let mut profiles: Option<Vec<Profile>> = None;
    let Ok(text) = fs::read_to_string(path) else {
        log.write(&format!("读取 {} 失败", path.display()));
        return out;
    };
    // 剥掉 UTF-8 BOM。记事本保存的 UTF-8 文件带 BOM（U+FEFF），而 Rust 的 trim() 不会去掉它
    // ——U+FEFF 不属于 Unicode 的 White_Space 属性。不剥的话第一条规则的程序名会带上 BOM，
    // 于是永远匹配不上任何前台程序，而且是静默失效。C# 版的 File.ReadAllLines 会自动剥 BOM。
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else {
            // 之前这里是静默跳过，用户少打一个等号就完全看不出问题
            log.write(&format!("这一行没有等号，已跳过: {line}"));
            continue;
        };
        let exe = line[..eq].trim().to_lowercase();
        if exe.is_empty() {
            log.write(&format!("这一行缺少程序名，已跳过: {line}"));
            continue;
        }
        let rhs = line[eq + 1..].trim();
        let (target, mode) = split_mode(rhs);
        if target.is_empty() {
            log.write(&format!("这一行缺少输入法，已跳过: {line}"));
            continue;
        }
        // 先按 TIP 解析（老格式）；失败再按输入法显示名查（新格式）
        let resolved = match parse_tip(target) {
            Ok(v) => Ok(v),
            Err(tip_err) => {
                let looks_tip = target.contains(':') || target.contains('{');
                lookup_by_name(tsf, &mut profiles, target).map_err(|msg| {
                    if looks_tip {
                        format!("{msg}；TIP 解析错误: {tip_err}")
                    } else {
                        msg
                    }
                })
            }
        };
        let (langid, clsid, profile) = match resolved {
            Ok(v) => v,
            Err(msg) => {
                // 例如 "TIP em"：末尾记号不是已知模式、但去掉它后是合法 TIP，
                // 说明只是模式写错了，给更直接的提示
                if mode.is_none() {
                    if let Some((head, last)) = target.rsplit_once(char::is_whitespace) {
                        if parse_tip(head.trim()).is_ok() {
                            log.write(&format!(
                                "中英文模式无法识别（「{last}」，只支持 zh/中文、en/英文），已跳过: {line}"
                            ));
                            continue;
                        }
                    }
                }
                log.write(&format!("规则解析失败，已跳过: {line}（{msg}）"));
                continue;
            }
        };
        // 同一个程序名只保留第一条（否则菜单里会重复列出，而 find() 只用第一条）
        if out.iter().any(|r: &Rule| r.exe == exe) {
            log.write(&format!("重复规则（{exe}），已忽略后一条: {line}"));
            continue;
        }
        let tip = format_tip(langid, &clsid, &profile);
        // 显示名加载时解析一次缓存住，菜单/日志展示不再每次调 COM
        let name = tsf
            .description(langid, &clsid, &profile)
            .unwrap_or_else(|| tip.clone());
        out.push(Rule {
            exe,
            langid,
            clsid,
            profile,
            tip,
            name,
            mode,
        });
    }
    out
}

pub fn find<'a>(rules: &'a [Rule], exe: &str) -> Option<&'a Rule> {
    if exe.is_empty() {
        return None;
    }
    let e = exe.to_lowercase();
    rules.iter().find(|r| r.exe == e)
}

#[cfg(test)]
mod tests {
    use super::{split_mode, ConvMode};

    #[test]
    fn split_target_and_mode() {
        assert_eq!(split_mode("微软拼音"), ("微软拼音", None));
        assert_eq!(
            split_mode("微软拼音 en"),
            ("微软拼音", Some(ConvMode::English))
        );
        // 名称里带空格：只把最后一个能识别的记号当模式
        assert_eq!(split_mode("Microsoft Pinyin"), ("Microsoft Pinyin", None));
        assert_eq!(
            split_mode("Microsoft Pinyin en"),
            ("Microsoft Pinyin", Some(ConvMode::English))
        );
        // 整体只有一个词、且和模式别名相同，不能拆成空目标
        assert_eq!(split_mode("中文"), ("中文", None));
        assert_eq!(split_mode("en"), ("en", None));
        // 不认识的尾部记号留给名称解析，不做猜测
        assert_eq!(split_mode("微软拼音 jp"), ("微软拼音 jp", None));

        let tip =
            "0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}";
        let line = format!("{tip} en");
        assert_eq!(split_mode(&line), (tip, Some(ConvMode::English)));
    }

    #[test]
    fn mode_tokens() {
        for t in ["zh", "ZH", "cn", "chinese", "中文"] {
            assert_eq!(ConvMode::parse(t), Some(ConvMode::Chinese), "{t}");
        }
        for t in ["en", "EN", "english", "英文"] {
            assert_eq!(ConvMode::parse(t), Some(ConvMode::English), "{t}");
        }
        assert_eq!(ConvMode::parse("jp"), None);
        assert_eq!(ConvMode::parse(""), None);
    }
}
