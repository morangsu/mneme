//! 生成「汉字 → 拼音首字母」紧凑表。
//!
//! 只为一个用途：让搜索支持输入 `mh` 命中「麻黄」。完整拼音（含声调、全拼）
//! 数据量大得多（`pinyin` crate 自带全表约数百 KB），而本项目只需要首字母，
//! 所以离线生成一张每字 1 字节的表，随二进制一起发布，运行时不引任何依赖。
//!
//! 用法（在仓库根目录）：
//! ```text
//! cargo run --manifest-path tools/pinyin-gen/Cargo.toml
//! ```
//! 产物：`crates/mneme-core/src/pinyin_initials.bin`（20,902 字节）。
//!
//! 编码：索引 = 码点 - 0x4E00，值 1..=26 表示 a..=z，0 表示未知（表外 / 非汉字）。

use pinyin::ToPinyin;
use std::io::Write;

const START: u32 = 0x4E00;
const END: u32 = 0x9FA5; // CJK 统一表意文字基本区上界

fn main() -> std::io::Result<()> {
    let count = (END - START + 1) as usize;
    let mut table = vec![0u8; count];
    let mut known = 0usize;
    let mut multi = 0usize;

    for cp in START..=END {
        let Some(ch) = char::from_u32(cp) else { continue };
        let mut buf = [0u8; 4];
        let s: &str = ch.encode_utf8(&mut buf);
        // 多音字取「第一个读音」，与常见输入法首选项一致；
        // 统计数量便于日后需要时改成多对多。
        let letters: Vec<&str> = s.to_pinyin().flatten().map(|p| p.plain()).collect();
        if letters.len() > 1 {
            multi += 1;
        }
        let Some(first) = letters.first().and_then(|p| p.chars().next()) else {
            continue;
        };
        let Some(lower) = first.to_lowercase().next().filter(|c| c.is_ascii_lowercase()) else {
            continue;
        };
        table[(cp - START) as usize] = lower as u8 - b'a' + 1;
        known += 1;
    }

    let out = std::path::Path::new("crates/mneme-core/src/pinyin_initials.bin");
    let mut f = std::fs::File::create(out)?;
    f.write_all(&table)?;

    println!("写入 {}：{} 字节", out.display(), table.len());
    println!("覆盖汉字 {known}/{count} 个（{} 个多音字取首读音）", multi);
    Ok(())
}
