use crate::{error::Result, memory::Memory, op::Op};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

/// append-only 设备分片日志。
///
/// 关键：**每台设备只写自己那一个文件** `events/<device>.jsonl`。
/// 同步时各分片互不覆盖，连文件级冲突都不存在，更不需要 CRDT。
pub struct EventLog {
    dir: PathBuf,
    device: String,
}

impl EventLog {
    pub fn open(root: &Path, device: &str) -> Result<Self> {
        let clean = sanitize(device);
        if clean.is_empty() {
            return Err(crate::error::Error::Device(device.to_string()));
        }
        let dir = root.join("events");
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            device: clean,
        })
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn file_for(&self, device: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", sanitize(device)))
    }

    pub fn list_files(&self) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        if !self.dir.exists() {
            return Ok(out);
        }
        for e in fs::read_dir(&self.dir)? {
            let p = e?.path();
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
        out.sort(); // 设备名定序 -> 跨设备重放结果确定
        Ok(out)
    }

    pub fn append(&self, ops: &[Op]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let path = self.file_for(&self.device);
        let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
        for op in ops {
            serde_json::to_writer(&mut f, op)?;
            f.write_all(b"\n")?;
        }
        f.flush()?;
        Ok(())
    }

    /// 重放。刻意做成「容错」：坏行/被截断的最后一行跳过而不是让整个库打不开
    /// —— 断电或同步中断时最后一行常是不完整的。
    pub fn replay(&self, mem: &mut Memory) -> Result<usize> {
        let mut applied = 0usize;
        for path in self.list_files()? {
            let f = fs::File::open(&path)?;
            for line in BufReader::new(f).lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => break,
                };
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(op) = serde_json::from_str::<Op>(&line) {
                    if mem.apply(&op) {
                        applied += 1;
                    }
                }
            }
        }
        Ok(applied)
    }

    /// 读出全部原始操作（文件名序 + 文件内顺序）。
    ///
    /// 与 `replay` 的差别：replay 只关心"最终状态"，会丢掉被幂等键去重的重复行；
    /// 历史和撤销要的是**操作序列**，所以这里不过 `Memory`，直接逐行反序列化。
    /// 容错策略与 replay 一致：坏行/截断行跳过。
    pub fn read_ops(&self) -> Result<Vec<Op>> {
        let mut out = Vec::new();
        for path in self.list_files()? {
            let f = fs::File::open(&path)?;
            for line in BufReader::new(f).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(op) = serde_json::from_str::<Op>(&line) {
                    out.push(op);
                }
            }
        }
        Ok(out)
    }

    pub fn read_device_bytes(&self, device: &str) -> Result<Vec<u8>> {
        let p = self.file_for(device);
        if !p.exists() {
            return Ok(Vec::new());
        }
        Ok(fs::read(p)?)
    }

    /// 合并远端设备分片：只在本地缺行时追加（本地分片永不覆盖）。
    pub fn merge_device_bytes(&self, device: &str, bytes: &[u8]) -> Result<usize> {
        let local = self.read_device_bytes(device)?;
        if local == bytes {
            return Ok(0);
        }
        // 只追加本地缺失的行，本地分片永不被远端覆盖
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for l in local.split(|b| *b == b'\n') {
            if !l.is_empty() {
                seen.insert(String::from_utf8_lossy(l).to_string());
            }
        }
        let mut added = 0usize;
        let mut extra = Vec::new();
        for l in bytes.split(|b| *b == b'\n') {
            if l.is_empty() {
                continue;
            }
            let s = String::from_utf8_lossy(l).to_string();
            if seen.insert(s.clone()) {
                extra.extend_from_slice(s.as_bytes());
                extra.push(b'\n');
                added += 1;
            }
        }
        if added > 0 {
            let path = self.file_for(device);
            let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
            f.write_all(&extra)?;
            f.flush()?;
        }
        Ok(added)
    }
}

pub fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if out.is_empty() { "default".into() } else { out }
}
