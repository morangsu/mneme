//! 卡片缓存：把派生出来的反向卡落盘，冷启动只做校验、不重算。
//!
//! 为什么需要：卡片生成是启动成本的大头（2000 味药实测 2.4 s，占派生 95%），
//! 而它是**纯派生数据**——同样的实体 + 同样的领域包 + 同样的分组，必然得到同样的卡。
//! 既然如此，就没有理由每次开窗都重算一遍。
//!
//! 为什么不用 JSON：34k 张卡约 10 MB，`serde_json` 逐条解析要 100 ms 以上，
//! 省下来的时间又还回去了。这里用定长头 + 长度前缀的紧凑二进制，实测读取 <40 ms。
//!
//! 为什么带「按实体的指纹」而不是一个全局版本号：
//! 全局版本号会让任何一次编辑都作废整个缓存，下次启动仍然 2.4 s。
//! 指纹要能表达「我这批卡依赖什么」——见 `memory::block_fp`：
//! 自己这条的 rev + 领域包 + 分组版本 + **同线索词的其他实体的 rev 与条数**
//! （因为「这条线索是否唯一」会影响题面与答案）。依赖变了才重算这一块。
//!
//! 缓存是**可重建的**：文件损坏、版本不符、指纹不匹配，一律当没有，重新生成。
//! 它和 `index.sqlite` 同级，都不是真相源，删掉不影响数据。

use crate::error::Result;
use mneme_core::Card;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"MNEMECRD";
const VERSION: u32 = 1;

/// 一个实体的卡片块 + 生成它时的依赖指纹。
#[derive(Debug, Clone)]
pub struct Block {
    pub fp: u64,
    pub cards: Vec<Card>,
}

#[derive(Debug, Clone, Default)]
pub struct CardCache {
    pub schema_fp: u64,
    pub blocks: HashMap<String, Block>,
}

pub fn path(root: &Path) -> PathBuf {
    root.join("cards.bin")
}

/// 读缓存。任何异常（文件不存在、magic 不符、截断、版本旧）都返回 `None`，
/// 让调用方走重算路径——缓存失效绝不能让程序起不来。
pub fn load(root: &Path) -> Option<CardCache> {
    let bytes = fs::read(path(root)).ok()?;
    if bytes.len() < 8 + 4 + 8 + 4 {
        return None;
    }
    let mut r = Reader { buf: &bytes, pos: 0 };
    if r.take(8)? != MAGIC {
        return None;
    }
    if r.u32()? != VERSION {
        return None;
    }
    let schema_fp = r.u64()?;
    let n = r.u32()? as usize;
    let mut blocks = HashMap::with_capacity(n);
    for _ in 0..n {
        let id = r.string()?;
        let fp = r.u64()?;
        let count = r.u32()? as usize;
        let mut cards = Vec::with_capacity(count);
        for _ in 0..count {
            cards.push(r.card()?);
        }
        blocks.insert(id, Block { fp, cards });
    }
    if r.pos != bytes.len() {
        // 尾部有多余字节说明写坏了（或并发写了一半），宁可重算
        return None;
    }
    Some(CardCache { schema_fp, blocks })
}

/// 写缓存（原子替换）。失败不返回错误：缓存写不进去只影响下次启动速度。
pub fn save(root: &Path, cache: &CardCache) -> Result<()> {
    let mut out: Vec<u8> = Vec::with_capacity(1 << 20);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&cache.schema_fp.to_le_bytes());
    out.extend_from_slice(&(cache.blocks.len() as u32).to_le_bytes());
    // 按实体 id 排序写出，便于出问题时肉眼比对、也让文件内容可复现
    let mut keys: Vec<&String> = cache.blocks.keys().collect();
    keys.sort();
    for k in keys {
        let b = &cache.blocks[k];
        put_str(&mut out, k);
        out.extend_from_slice(&b.fp.to_le_bytes());
        out.extend_from_slice(&(b.cards.len() as u32).to_le_bytes());
        for c in &b.cards {
            put_card(&mut out, c);
        }
    }
    let tmp = path(root).with_extension("bin.tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(&out)?;
    f.sync_all().ok();
    fs::rename(&tmp, path(root))?;
    Ok(())
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn put_strs(out: &mut Vec<u8>, v: &[String]) {
    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
    for s in v {
        put_str(out, s);
    }
}

fn put_card(out: &mut Vec<u8>, c: &Card) {
    put_str(out, &c.id);
    put_str(out, &c.entity_id);
    put_str(out, &c.direction);
    put_str(out, &c.clue_field);
    put_str(out, &c.clue_term);
    put_str(out, &c.prompt);
    put_str(out, &c.answer);
    put_strs(out, &c.accept);
    put_strs(out, &c.answer_fields);
    match &c.scope {
        Some(s) => {
            out.push(1);
            put_str(out, s);
        }
        None => out.push(0),
    }
    put_strs(out, &c.candidates);
    out.push(c.enumerate as u8);
    put_strs(out, &c.immutable_boundary);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        if end > self.buf.len() {
            return None;
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Some(s)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| {
            u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
        })
    }

    fn string(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let b = self.take(n)?;
        // 先校验再复制：`String::from_utf8(b.to_vec())` 会先拷一遍再验，
        // 3 万张卡就是白白多拷几 MB
        std::str::from_utf8(b).ok().map(str::to_owned)
    }

    fn strings(&mut self) -> Option<Vec<String>> {
        let n = self.u32()? as usize;
        // 防御：一个坏长度可能让这里试着分配几十 GB
        if n > 1_000_000 {
            return None;
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(self.string()?);
        }
        Some(v)
    }

    fn card(&mut self) -> Option<Card> {
        Some(Card {
            id: self.string()?,
            entity_id: self.string()?,
            direction: self.string()?,
            clue_field: self.string()?,
            clue_term: self.string()?,
            prompt: self.string()?,
            answer: self.string()?,
            accept: self.strings()?,
            answer_fields: self.strings()?,
            scope: if self.u8()? == 1 { Some(self.string()?) } else { None },
            candidates: self.strings()?,
            enumerate: self.u8()? != 0,
            immutable_boundary: self.strings()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, n: usize) -> Card {
        Card {
            id: id.into(),
            entity_id: "e".into(),
            direction: "d".into(),
            clue_field: "f".into(),
            clue_term: "词".into(),
            prompt: format!("题面 {n}"),
            answer: "答案".into(),
            accept: vec!["甲".into(), "乙".into()],
            answer_fields: vec!["name".into()],
            scope: if n % 2 == 0 { Some("药群".into()) } else { None },
            candidates: vec!["丙".into()],
            enumerate: n % 3 == 0,
            immutable_boundary: vec!["原样".into()],
        }
    }

    #[test]
    fn roundtrip_keeps_every_field() {
        let dir = std::env::temp_dir().join(format!("mneme-cardcache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut cache = CardCache { schema_fp: 42, blocks: HashMap::new() };
        cache.blocks.insert("a".into(), Block { fp: 7, cards: (0..5).map(|n| card(&format!("a{n}"), n)).collect() });
        cache.blocks.insert("b".into(), Block { fp: 8, cards: vec![card("b0", 9)] });
        save(&dir, &cache).unwrap();

        let got = load(&dir).expect("load");
        assert_eq!(got.schema_fp, 42);
        assert_eq!(got.blocks.len(), 2);
        let a = &got.blocks["a"];
        assert_eq!(a.fp, 7);
        assert_eq!(a.cards.len(), 5);
        // 逐字段比对（含 scope 的 Some/None 与 bool）
        for n in 0..5 {
            assert_eq!(a.cards[n], cache.blocks["a"].cards[n], "第 {n} 张卡不一致");
        }
        // n=0 是偶数 -> Some，n=1 是奇数 -> None（确认 Option 的两种形态都活着）
        assert_eq!(got.blocks["a"].cards[0].scope.as_deref(), Some("药群"));
        assert!(got.blocks["a"].cards[1].scope.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_or_missing_returns_none() {
        let dir = std::env::temp_dir().join(format!("mneme-cardcache-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(load(&dir).is_none(), "没有文件就该返回 None");

        // 截断的文件
        fs::write(path(&dir), b"MNEMECRD\x01\x00").unwrap();
        assert!(load(&dir).is_none(), "截断不能 panic");

        // 尾部多字节（并发写坏的典型形态）
        let mut cache = CardCache { schema_fp: 1, blocks: HashMap::new() };
        cache.blocks.insert("x".into(), Block { fp: 1, cards: vec![card("x", 1)] });
        save(&dir, &cache).unwrap();
        let mut bytes = fs::read(path(&dir)).unwrap();
        bytes.push(0);
        fs::write(path(&dir), &bytes).unwrap();
        assert!(load(&dir).is_none(), "长度对不上就该丢弃");
        let _ = fs::remove_dir_all(&dir);
    }
}
