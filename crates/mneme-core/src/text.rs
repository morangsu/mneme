use crate::schema::TermAliasDef;
use std::collections::HashMap;

/// 剥掉 OCR / 题库常见的「A.」「12、」「(3)」前缀，压缩空白。
/// 移植自 tiku/src/normalize.mjs，纯手写状态机，不引 regex。
pub fn remove_redundant(s: &str) -> String {
    let trimmed = s.trim();
    // 快路径：没有「A.」/「12、」前缀、也没有多余空白时，原样返回。
    // 索引重建、检索、卡片指纹都在这条函数上跑几十万次，省掉两次全量拷贝很值。
    let first = trimmed.chars().next();
    let may_have_prefix = matches!(first, Some(c) if c.is_ascii_uppercase() || c.is_ascii_digit());
    let has_ws = trimmed.chars().any(|c| c.is_whitespace());
    if !may_have_prefix && !has_ws {
        return trimmed.to_string();
    }
    let mut chars: Vec<char> = trimmed.chars().collect();
    if chars.len() >= 2 && chars[0].is_ascii_uppercase() {
        if matches!(chars[1], '.' | '、' | ')' | '．' | ':' | '：') {
            chars.drain(0..2);
        }
    }
    let n = chars.iter().take_while(|c| c.is_ascii_digit()).count();
    if n > 0 && n <= 3 && n < chars.len() {
        if matches!(chars[n], '.' | '、' | ')' | '．' | ':' | '：') {
            let mut end = n + 1;
            while end < chars.len() && chars[end].is_whitespace() {
                end += 1;
            }
            chars.drain(0..end);
        }
    }
    let joined: String = chars.into_iter().collect();
    joined
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 中文归一化：全角转半角、大写转小写、只保留字母数字与 CJK。
/// 覆盖 0x3400-0x9FFF（含扩展 A，中药生僻字如「䗪」）与扩展 B 之后。
pub fn normalize(s: &str) -> String {
    normalize_mapped(s).0
}

/// 归一化 + 原文位置映射。
///
/// 返回 `(归一化串, 每个归一化字符对应的原文 char 下标)`。
/// 搜索要给出「命中片段」，但归一化会丢掉标点，位置就对不上了；
/// 有了映射才能从归一化串里的命中位置反推回原文，截一段带标点的上下文。
pub fn normalize_mapped(s: &str) -> (String, Vec<usize>) {
    let cleaned = remove_redundant(s);
    let mut out = String::with_capacity(cleaned.len());
    let mut map = Vec::with_capacity(cleaned.len());
    for (idx, ch) in cleaned.chars().enumerate() {
        if let Some(c) = keep_char(ch) {
            out.push(c);
            map.push(idx);
        }
    }
    (out, map)
}

/// 单个字符的归一化规则：全角转半角、大写转小写、只保留字母数字与 CJK。
fn keep_char(ch: char) -> Option<char> {
    let cp = ch as u32;
    let c = if (0xFF21..=0xFF3A).contains(&cp)
        || (0xFF41..=0xFF5A).contains(&cp)
        || (0xFF10..=0xFF19).contains(&cp)
    {
        char::from_u32(cp - 0xFEE0).unwrap_or(ch)
    } else if cp == 0xFF05 {
        '%'
    } else {
        ch
    };
    let c = c.to_ascii_lowercase();
    let u = c as u32;
    // 只保留汉字与部首。刻意跳过 0x3000-0x33FF（CJK 标点 / 假名），
    // 否则「发汗解表、宣肺平喘」里的顿号会进索引，导致同一术语两种键。
    let cjk = (0x2E80..=0x2FDF).contains(&u)
        || (0x3400..=0x9FFF).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0x20000..=0x323AF).contains(&u);
    if c.is_ascii_alphanumeric() || cjk { Some(c) } else { None }
}

/// `remove_redundant` 的零拷贝版：只返回「该从哪里开始」的字节偏移。
///
/// 检索要对每条实体的每个字段做一次「归一化后是否包含关键词」，如果每次都
/// `to_string()` 一遍，2000 味药 × 21 字段就是 4 万次分配——比真正的比较还贵。
fn head_skip(s: &str) -> usize {
    let mut it = s.char_indices();
    let Some((_, first)) = it.next() else { return 0 };
    if first.is_ascii_uppercase() {
        if let Some((i, second)) = it.next() {
            if is_sep(second) {
                return i + second.len_utf8();
            }
        }
        return 0;
    }
    if !first.is_ascii_digit() {
        return 0;
    }
    // 1~3 位数字 + 一个分隔符
    let mut digits = 1usize;
    for (i, c) in it {
        if c.is_ascii_digit() && digits < 3 {
            digits += 1;
            continue;
        }
        return if is_sep(c) { i + c.len_utf8() } else { 0 };
    }
    0
}

fn is_sep(c: char) -> bool {
    matches!(c, '.' | '、' | ')' | '．' | ':' | '：')
}

/// 归一化子串匹配器：模式只归一化/建前缀表一次，对多条文本复用。
///
/// 语义与 `normalize(haystack).contains(normalize(needle))` 一致（needle 由调用方先归一化），
/// 区别是逐字符走 `keep_char` 流式比较，不构造中间字符串；前缀表也只建一次，
/// 否则「2000 味药 × 21 字段」就是 4 万次分配，比真正的比较还贵。
pub struct NormMatcher {
    pat: Vec<char>,
    fail: Vec<usize>,
}

impl NormMatcher {
    pub fn new(needle: &str) -> Self {
        let pat: Vec<char> = needle.chars().collect();
        let mut fail = vec![0usize; pat.len()];
        for i in 1..pat.len() {
            let mut k = fail[i - 1];
            while k > 0 && pat[i] != pat[k] {
                k = fail[k - 1];
            }
            if pat[i] == pat[k] {
                k += 1;
            }
            fail[i] = k;
        }
        Self { pat, fail }
    }

    pub fn is_empty(&self) -> bool {
        self.pat.is_empty()
    }

    pub fn is_match(&self, haystack: &str) -> bool {
        if self.pat.is_empty() {
            return true;
        }
        let s = haystack.trim();
        let start = head_skip(s);
        let mut matched = 0usize;
        for (i, ch) in s.char_indices() {
            if i < start {
                continue;
            }
            let Some(c) = keep_char(ch) else { continue };
            while matched > 0 && self.pat[matched] != c {
                matched = self.fail[matched - 1];
            }
            if self.pat[matched] == c {
                matched += 1;
                if matched == self.pat.len() {
                    return true;
                }
            }
        }
        false
    }
}

/// 一次性版本（单次调用用；批量场景请复用 `NormMatcher`）。
pub fn contains_normalized(haystack: &str, needle: &str) -> bool {
    NormMatcher::new(needle).is_match(haystack)
}

/// Dice 双字组系数，与 tiku 的 compareTwoStrings 同算法。
pub fn dice(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    if ac.len() < 2 || bc.len() < 2 {
        return 0.0;
    }
    let mut counts: HashMap<(char, char), i32> = HashMap::with_capacity(ac.len());
    for w in ac.windows(2) {
        *counts.entry((w[0], w[1])).or_insert(0) += 1;
    }
    let mut hit = 0i32;
    for w in bc.windows(2) {
        if let Some(c) = counts.get_mut(&(w[0], w[1])) {
            if *c > 0 {
                *c -= 1;
                hit += 1;
            }
        }
    }
    (2.0 * hit as f64) / ((ac.len() + bc.len() - 2) as f64)
}

/// 判分用：先归一化，再取包含关系 / Dice 的最大值。
pub fn similarity(a: &str, b: &str) -> f64 {
    let (na, nb) = (normalize(a), normalize(b));
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    if na == nb {
        return 1.0;
    }
    if na.contains(&nb) || nb.contains(&na) {
        return 0.95;
    }
    dice(&na, &nb)
}

/// 同义表述表：教材主表述唯一，同义说法只作为可命中关键词。
/// `boundary` 存「不能替换的限定」，出题器对它不做替换。
#[derive(Debug, Clone, Default)]
pub struct TermTable {
    map: HashMap<String, String>,
    boundaries: HashMap<String, String>,
}

impl TermTable {
    pub fn from_defs(defs: &[TermAliasDef]) -> Self {
        let mut t = Self::default();
        for d in defs {
            let canonical = d.canonical.clone();
            t.map.insert(normalize(&d.concept), canonical.clone());
            t.map.insert(normalize(&canonical), canonical.clone());
            for a in &d.aliases {
                t.map.insert(normalize(a), canonical.clone());
            }
            if let Some(b) = &d.boundary {
                t.boundaries.insert(normalize(&d.concept), b.clone());
            }
        }
        t
    }

    pub fn canonical(&self, s: &str) -> String {
        self.map.get(&normalize(s)).cloned().unwrap_or_else(|| s.to_string())
    }

    pub fn boundary(&self, concept: &str) -> Option<&str> {
        self.boundaries.get(&normalize(concept)).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_handles_fullwidth_and_punctuation() {
        assert_eq!(normalize("ＡＢＣ１２３"), "abc123");
        assert_eq!(normalize("细胞膜（半透膜）！"), "细胞膜半透膜");
        assert_eq!(normalize("发汗解表、宣肺平喘"), "发汗解表宣肺平喘");
    }

    #[test]
    fn normalize_keeps_rare_cjk() {
        assert!(normalize("䗪虫").contains("䗪"));
    }

    #[test]
    fn strips_option_and_number_prefix() {
        assert_eq!(remove_redundant("A. 细胞膜"), "细胞膜");
        assert_eq!(remove_redundant("12、下列关于"), "下列关于");
    }

    #[test]
    fn dice_and_similarity() {
        assert_eq!(dice("abcd", "abcd"), 1.0);
        assert!(dice("细胞膜的选择透过性", "细胞膜的选择透过") > 0.5);
        assert_eq!(similarity("“发散表邪”。", "发散表邪"), 1.0);
        assert!(similarity("发散表邪、解除表证", "发散表邪") > 0.9);
        // 无关词不该硬凑：同义关系由 TermTable 显式声明，不靠模糊匹配
        assert_eq!(similarity("发散表邪", "解表"), 0.0);
    }

    #[test]
    fn term_table_canonicalizes() {
        let t = TermTable::from_defs(&[TermAliasDef {
            concept: "解表药".into(),
            canonical: "以发散表邪、解除表证为主要功效".into(),
            aliases: vec!["发散表邪".into(), "解表".into()],
            boundary: Some("不能写成发汗解肌作总定义".into()),
            page: Some(12),
        }]);
        assert!(t.canonical("解表").starts_with("以发散表邪"));
        assert!(t.boundary("解表药").is_some());
    }

    #[test]
    fn contains_normalized_ignores_punctuation() {
        assert!(contains_normalized("辛、微苦，温。归肺、膀胱经", "辛微苦"));
        assert!(contains_normalized("发汗解表、宣肺平喘", "宣肺平喘"));
        assert!(contains_normalized("A.发汗解表", "发汗解表"));
        assert!(contains_normalized("12、利水消肿", "利水消肿"));
        assert!(!contains_normalized("发汗解表", "清热"));
        // 全角字母数字也要能对上
        assert!(contains_normalized("ＡＢＣ１２３", "abc123"));
    }

    #[test]
    fn contains_normalized_agrees_with_normalize() {
        // 与「先归一化再 contains」保持同语义（否则索引和检索会分叉）
        let cases = [
            ("辛、微苦，温。归肺、膀胱经", "膀胱"),
            ("3–10g", "10g"),
            ("发汗解表、宣肺平喘", "汗解"),
            ("麻黄（龙沙）", "龙沙"),
        ];
        for (hay, needle) in cases {
            let expect = normalize(hay).contains(&normalize(needle));
            assert_eq!(contains_normalized(hay, &normalize(needle)), expect, "{hay} / {needle}");
        }
    }
}
