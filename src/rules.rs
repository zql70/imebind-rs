//! rules.txt 的解析与查找
//!
//! 格式（和 C# 版一致，程序名不区分大小写，`#` 开头为注释）：
//!   dota2.exe = 0804:{CLSID}{ProfileGUID}
use crate::log::Log;
use crate::tsf::parse_tip;
use std::fs;
use std::path::Path;
use windows::core::GUID;

#[derive(Clone, Debug)]
pub struct Rule {
    pub exe: String,
    pub langid: u16,
    pub clsid: GUID,
    pub profile: GUID,
    /// 规范化后的 TIP 字符串（大写），用于和当前输入法比较
    pub tip: String,
}

impl Rule {
    pub fn display(&self, tsf: &crate::tsf::Tsf) -> String {
        let name = tsf
            .description(self.langid, &self.clsid, &self.profile)
            .unwrap_or_else(|| self.tip.clone());
        format!("{} → {}", self.exe, name)
    }
}

/// 文件不存在时写一份带注释的示例
pub fn ensure_example(path: &Path) {
    if path.exists() {
        return;
    }
    let _ = fs::write(
        path,
        "# ImeBind 规则：每行 \"程序名 = 输入法TIP\"，程序名不区分大小写\r\n\
         # 本机可用的 TIP 用 imebind --list 查看\r\n\
         # 示例（微软拼音）：\r\n\
         # dota2.exe = 0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}\r\n",
    );
}

pub fn load(path: &Path, log: &Log) -> Vec<Rule> {
    let mut out = Vec::new();
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
        let tip_raw = line[eq + 1..].trim();
        match parse_tip(tip_raw) {
            Ok((langid, clsid, profile)) => {
                // 同一个程序名只保留第一条（否则菜单里会重复列出，而 find() 只用第一条）
                if out.iter().any(|r: &Rule| r.exe == exe) {
                    log.write(&format!("重复规则（{exe}），已忽略后一条: {line}"));
                    continue;
                }
                out.push(Rule {
                    exe,
                    langid,
                    clsid,
                    profile,
                    tip: crate::tsf::format_tip(langid, &clsid, &profile),
                });
            }
            Err(e) => log.write(&format!("规则解析失败，已跳过: {line} （{e}）")),
        }
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
