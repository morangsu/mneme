//! 进程内凭据注册表：让配置里只写 `${SECRET:名字}`，真值不落盘。
//!
//! 为什么不是加密文件：一台设备上「能解密文件」和「能读配置文件」是同一权限，
//! 自造文件加密只是把密码换个地方放。真正的边界在系统能力上——
//! Android 用 Keystore（密钥不出安全硬件，密文即使被读走也解不开），
//! 桌面端可以用系统钥匙串或环境变量。所以 Rust 侧只做**内存**注册表：
//! 宿主在启动时把凭据塞进来，进程退出就没了；磁盘上永远只有引用名。
//!
//! 契约：
//! - `put` 只写内存，不写文件、不打印、不进日志。
//! - `names()` 只返回名字，绝不返回值——UI 用它显示「已配置 / 未配置」。
//! - 名字为空串直接拒绝，避免出现「空引用意外命中所有目标」。

use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};

static SECRETS: LazyLock<RwLock<BTreeMap<String, String>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

fn guard() -> std::sync::RwLockWriteGuard<'static, BTreeMap<String, String>> {
    // 中毒也不放弃：注册表里没有跨字段不变量，读到别人写了一半的状态最多是
    // 「这次同步说凭据缺失」，比整个 APP 崩掉好。
    SECRETS.write().unwrap_or_else(|e| e.into_inner())
}

fn read() -> std::sync::RwLockReadGuard<'static, BTreeMap<String, String>> {
    SECRETS.read().unwrap_or_else(|e| e.into_inner())
}

/// 名称合法性：只允许字母数字与 `-` `_` `.`，长度 1..=64。
/// 限制字符集是为了让 `${SECRET:xxx}` 这个语法本身无歧义（不能含 `}`）。
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn put(name: &str, value: &str) -> bool {
    if !valid_name(name) {
        return false;
    }
    guard().insert(name.to_string(), value.to_string());
    true
}

pub fn remove(name: &str) -> bool {
    guard().remove(name).is_some()
}

pub fn get(name: &str) -> Option<String> {
    read().get(name).cloned()
}

/// 已配置的凭据名（不含值，可安全给 UI 与日志）。
pub fn names() -> Vec<String> {
    read().keys().cloned().collect()
}

pub fn has(name: &str) -> bool {
    read().contains_key(name)
}

/// 清空（测试用；也用于「退出登录」）。
pub fn clear() {
    guard().clear();
}

/// 字符串里出现这些前缀，说明有人把真 token 拼进了要打印的东西。
pub const SECRET_PREFIXES: [&str; 4] = ["ghp_", "github_pat_", "gho_", "ghs_"];

/// 应急脱敏：把形如真 token 的子串替换成 `***`。用于任何必须输出自由文本的兜底。
pub fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for p in SECRET_PREFIXES {
        while let Some(i) = out.find(p) {
            let end = out[i..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ')' | '}'))
                .map(|k| i + k)
                .unwrap_or(out.len());
            out.replace_range(i..end, "***");
        }
    }
    out
}

/// 注册表是进程级全局状态，多个测试并行跑会互相 clear。
/// 用到注册表的测试统一拿这把锁，串行执行。
#[cfg(test)]
pub(crate) fn serialize_tests() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_remove_roundtrip() {
        let _g = serialize_tests();
        clear();
        assert!(put("github", "ghp_secret"));
        assert_eq!(get("github").as_deref(), Some("ghp_secret"));
        assert_eq!(names(), vec!["github".to_string()]);
        assert!(remove("github"));
        assert!(get("github").is_none());
        clear();
    }

    #[test]
    fn rejects_bad_names() {
        let _g = serialize_tests();
        clear();
        assert!(!put("", "x"));
        assert!(!put("有中文", "x"));
        // 含 `}` 会破坏 ${SECRET:...} 语法
        assert!(!put("a}b", "x"));
        assert!(put("ok-name_1.x", "x"));
        clear();
    }

    #[test]
    fn redact_kills_token_like_substrings() {
        let s = "sync failed token=ghp_abc123XYZ more";
        let r = redact(s);
        assert!(!r.contains("ghp_"));
        assert!(r.contains("***"));
        assert!(r.contains("more"));
    }
}
