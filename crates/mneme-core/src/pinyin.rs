//! 汉字 → 拼音首字母。
//!
//! 只做一件事：让搜索支持「输入 mh 命中麻黄」。完整拼音（全拼/声调）数据量大一个
//! 数量级，本项目的输入场景是「手敲首字母」，所以只带一张覆盖 CJK 基本区的首字母表。
//!
//! 表的来源与重生成方式见 `tools/pinyin-gen`：20,902 字节，每字 1 字节，
//! 索引 = 码点 - 0x4E00，值 1..=26 表示 a..=z，0 表示未知（多音字取第一个读音）。
//! 表外字符（扩展 A、扩展 B 之后的生僻字）不参与首字母匹配，但**仍可被普通子串搜到**。

const TABLE: &[u8] = include_bytes!("pinyin_initials.bin");
const START: u32 = 0x4E00;
const END: u32 = 0x9FA5;

/// 单字首字母；表外或未知返回 `None`。
pub fn initial_of(ch: char) -> Option<char> {
    let cp = ch as u32;
    if !(START..=END).contains(&cp) {
        return None;
    }
    let v = TABLE[(cp - START) as usize];
    if v == 0 {
        None
    } else {
        Some((b'a' + v - 1) as char)
    }
}

/// 取一段文本的拼音首字母串。
///
/// 汉字取首字母，ASCII 字母原样小写保留，其余（标点、数字、空白、生僻字）跳过——
/// 这样「麻黄（龙沙）」的首字母是 `mhls`，用户先敲 `mh` 就能命中。
pub fn initials(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphabetic() {
            out.push(ch.to_ascii_lowercase());
        } else if let Some(c) = initial_of(ch) {
            out.push(c);
        }
    }
    out
}

/// 首字母串里是否含 `needle`（needle 必须全 ASCII 字母，由调用方保证）。
///
/// 返回匹配类型：前缀命中比中间命中更可信，评分时区分。
pub fn initials_match(haystack: &str, needle: &str) -> Option<bool> {
    if needle.is_empty() || !needle.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    if haystack.starts_with(needle) {
        Some(true)
    } else if haystack.contains(needle) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_of_common_herbs() {
        assert_eq!(initials("麻黄"), "mh");
        assert_eq!(initials("桂枝"), "gz");
        assert_eq!(initials("苦杏仁"), "kxr");
        assert_eq!(initials("紫苏"), "zs");
        // 标点、括号、数字都不进首字母串
        assert_eq!(initials("麻黄（龙沙）"), "mhls");
        assert_eq!(initials("2–10g"), "g");
    }

    #[test]
    fn table_covers_basic_block_but_not_beyond() {
        // 扩展 A 的表外字不崩、返回 None，搜索仍可走子串
        assert_eq!(initial_of('㐀'), None);
        assert!(initial_of('麻').is_some());
        // 表里没有的编码不 panic
        assert_eq!(initial_of('A'), None);
    }

    #[test]
    fn match_prefers_prefix() {
        assert_eq!(initials_match("mhls", "mh"), Some(true));
        assert_eq!(initials_match("mhls", "ls"), Some(false));
        assert_eq!(initials_match("mhls", "zz"), None);
        // 非纯小写字母的 needle 直接不参与（避免中文/数字误判）
        assert_eq!(initials_match("mh", "M"), None);
    }
}
