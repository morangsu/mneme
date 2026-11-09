use mneme_core::{
    Card, DomainSchema, Entity, FieldKind, Group, ReviewEvent, TermAlias, Value, content_hash,
};
use mneme_store::Snapshot;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// 所有格式共用的中间表示。格式层只负责「字节 <-> Payload」，
/// 不碰 store、不知道同步，导入策略由 lib.rs 统一裁决。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub schema_toml: String,
    pub entities: Vec<Entity>,
    pub groups: Vec<Group>,
    pub aliases: Vec<TermAlias>,
    pub events: Vec<ReviewEvent>,
}

impl Payload {
    pub fn from_snapshot(s: &Snapshot) -> Self {
        Self {
            schema_toml: s.schema_toml.clone(),
            entities: s.entities.clone(),
            groups: s.groups.clone(),
            aliases: s.aliases.clone(),
            events: s.events.clone(),
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn checksum(&self) -> String {
        content_hash(&self.canonical_bytes())
    }
}

pub const BUNDLE_FORMAT: &str = "mneme-bundle/1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub format: String,
    pub version: u32,
    pub created_at: i64,
    pub generator: String,
    pub payload: Payload,
    pub checksum: String,
}

impl Bundle {
    pub fn new(p: Payload, now: i64) -> Self {
        let mut b = Self {
            format: BUNDLE_FORMAT.into(),
            version: 1,
            created_at: now,
            generator: "mneme-codec".into(),
            payload: p,
            checksum: String::new(),
        };
        b.checksum = b.compute();
        b
    }

    fn compute(&self) -> String {
        let mut c = self.clone();
        c.checksum.clear();
        content_hash(&serde_json::to_vec(&c).unwrap_or_default())
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.format != BUNDLE_FORMAT {
            return Err(format!("未知格式 {}", self.format));
        }
        if self.compute() != self.checksum {
            return Err("Bundle 校验和不匹配".into());
        }
        Ok(())
    }
}

pub fn to_json(p: &Payload) -> std::result::Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(p)
}

pub fn from_json(b: &[u8]) -> std::result::Result<Payload, serde_json::Error> {
    serde_json::from_slice(b)
}

pub fn to_bundle(p: &Payload, now: i64) -> std::result::Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(&Bundle::new(p.clone(), now))
}

pub fn from_bundle(b: &[u8], verify: bool) -> std::result::Result<Payload, String> {
    let bundle: Bundle = serde_json::from_slice(b).map_err(|e| e.to_string())?;
    if verify {
        bundle.verify()?;
    }
    Ok(bundle.payload)
}

fn csv_cell(s: &str, sep: char) -> String {
    if s.contains(sep) || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn parse_line(line: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if quoted {
            if c == '"' {
                if it.peek() == Some(&'"') {
                    it.next();
                    cur.push('"');
                } else {
                    quoted = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            quoted = true;
        } else if c == sep {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// 列顺序来自 schema，不受实体字段出现顺序影响 -> 导出字节可复现。
fn columns(schema: &DomainSchema) -> Vec<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for e in &schema.entities {
        for f in &e.fields {
            keys.insert(f.key.clone());
        }
    }
    let mut v = vec![
        "id".into(),
        "kind".into(),
        "group_id".into(),
        "deleted".into(),
        "rev".into(),
        "page".into(),
    ];
    v.extend(keys);
    v
}

pub fn to_delimited(p: &Payload, sep: char, schema: &DomainSchema) -> Vec<u8> {
    let cols = columns(schema);
    let mut lines = Vec::with_capacity(p.entities.len() + 1);
    lines.push(
        cols.iter()
            .map(|c| csv_cell(c, sep))
            .collect::<Vec<_>>()
            .join(&sep.to_string()),
    );
    for e in &p.entities {
        let row: Vec<String> = cols
            .iter()
            .map(|c| match c.as_str() {
                "id" => e.id.clone(),
                "kind" => e.kind.clone(),
                "group_id" => e.group_id.clone().unwrap_or_default(),
                "deleted" => e.deleted.to_string(),
                "rev" => e.rev.to_string(),
                "page" => e
                    .source
                    .as_ref()
                    .and_then(|s| s.page)
                    .map(|x| x.to_string())
                    .unwrap_or_default(),
                // 表格里 Tags 固定用 `|` 分隔；Markdown/展示才用顿号
                k => match e.get(k) {
                    Some(Value::Tags(v)) => v.join("|"),
                    Some(v) => v.display(),
                    None => String::new(),
                },
            })
            .map(|s| csv_cell(&s, sep))
            .collect();
        lines.push(row.join(&sep.to_string()));
    }
    let mut s = lines.join("\n");
    s.push('\n');
    // UTF-8 BOM：Excel 直接双击打开不乱码
    let mut out = vec![0xEF, 0xBB, 0xBF];
    out.extend_from_slice(s.as_bytes());
    out
}

pub fn from_delimited(
    b: &[u8],
    sep: char,
    schema: &DomainSchema,
) -> std::result::Result<Payload, String> {
    let text = String::from_utf8_lossy(b).trim_start_matches('\u{feff}').to_string();
    let mut lines = text.lines();
    let header = parse_line(lines.next().unwrap_or(""), sep);
    if !header.iter().any(|h| h == "id") {
        return Err("表头缺少 id 列".into());
    }
    let field_kinds: BTreeMap<String, FieldKind> = schema
        .entities
        .iter()
        .flat_map(|e| e.fields.iter().map(|f| (f.key.clone(), f.kind)))
        .collect();
    let mut entities = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let cells = parse_line(line, sep);
        let get = |k: &str| -> String {
            header
                .iter()
                .position(|h| h == k)
                .and_then(|i| cells.get(i))
                .cloned()
                .unwrap_or_default()
        };
        let id = get("id");
        if id.is_empty() {
            continue;
        }
        let kind = get("kind");
        let mut e = Entity::new(
            id.clone(),
            if kind.is_empty() { "entity".into() } else { kind },
            "import",
            0,
        );
        e.rev = get("rev").parse().unwrap_or(1);
        let gid = get("group_id");
        if !gid.is_empty() {
            e.group_id = Some(gid);
        }
        e.deleted = get("deleted") == "true";
        if let Ok(page) = get("page").parse::<u32>() {
            e.source = Some(mneme_core::SourceAnchor {
                page: Some(page),
                ..Default::default()
            });
        }
        for (i, col) in header.iter().enumerate() {
            let Some(&fk) = field_kinds.get(col) else { continue };
            let Some(raw) = cells.get(i) else { continue };
            if raw.is_empty() {
                continue;
            }
            let v = match fk {
                FieldKind::Tags => Value::Tags(raw.split('|').map(String::from).collect()),
                FieldKind::Number => Value::Number(raw.parse().unwrap_or(0.0)),
                FieldKind::Bool => Value::Bool(raw == "true"),
                _ => Value::Text(raw.clone()),
            };
            e.fields.insert(col.clone(), v);
        }
        entities.push(e);
    }
    Ok(Payload {
        entities,
        ..Default::default()
    })
}

pub fn to_markdown(p: &Payload, schema: &DomainSchema) -> Vec<u8> {
    let mut groups: BTreeMap<String, Vec<&Entity>> = BTreeMap::new();
    for e in &p.entities {
        groups
            .entry(e.group_id.clone().unwrap_or_else(|| "未分组".into()))
            .or_default()
            .push(e);
    }
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", schema.name));
    out.push_str("> 由 mneme 导出。教材主表述以实体书为准，页码见每条。\n\n");
    for (g, list) in groups {
        out.push_str(&format!("## {g}\n\n"));
        for e in list {
            let primary = schema
                .entity_def(&e.kind)
                .and_then(|d| d.primary.as_deref())
                .unwrap_or("name");
            out.push_str(&format!("### {}\n\n", e.display_of(primary)));
            for (k, v) in &e.fields {
                if k == primary || v.is_empty() {
                    continue;
                }
                let label = schema
                    .field(&e.kind, k)
                    .map(|f| f.label.as_str())
                    .unwrap_or(k.as_str());
                let mark = if schema.is_immutable(&e.kind, k) { "（不可替换）" } else { "" };
                out.push_str(&format!("- **{label}**{mark}：{}\n", v.display()));
            }
            if let Some(page) = e.source.as_ref().and_then(|s| s.page) {
                out.push_str(&format!("- 页码：{page}\n"));
            }
            out.push('\n');
        }
    }
    out.into_bytes()
}

pub fn to_anki(cards: &[Card]) -> Vec<u8> {
    let mut out = String::from("#separator:Tab\n#html:true\n#tags column:3\n");
    for c in cards {
        let clean = |s: &str| s.replace('\t', " ").replace('\n', "<br>").replace('\r', "");
        out.push_str(&format!(
            "{}\t{}\t{}\n",
            clean(&c.prompt),
            clean(&c.answer),
            clean(&c.direction)
        ));
    }
    out.into_bytes()
}

/// 单文件离线包：数据内嵌 + 内联 JS，安卓存到本地用浏览器打开即可复习。
/// 不需要 Service Worker、不需要联网、不需要装 App。
pub fn to_offline_html(p: &Payload, cards: &[Card]) -> Vec<u8> {
    #[derive(Serialize)]
    struct Offline<'a> {
        entities: &'a [Entity],
        groups: &'a [Group],
        cards: &'a [Card],
        schema_toml: &'a str,
    }
    let data = serde_json::to_string(&Offline {
        entities: &p.entities,
        groups: &p.groups,
        cards,
        schema_toml: &p.schema_toml,
    })
    .unwrap_or_else(|_| "{}".into())
    .replace('<', "\\u003c");
    format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>mneme 离线复习</title><style>body{{background:#16171a;color:#e6e7ea;font:15px/1.7 -apple-system,"Microsoft YaHei",sans-serif;margin:0;padding:16px;max-width:720px;margin:auto}}h1{{font-size:17px}}button{{background:#2b4b78;color:#e6e7ea;border:1px solid #3d6ba8;border-radius:8px;padding:10px 16px;font-size:15px;margin:6px 4px 6px 0;cursor:pointer}}#q{{background:#1e2024;border:1px solid #33363d;border-radius:12px;padding:18px;margin:14px 0;min-height:110px;white-space:pre-wrap}}#a{{background:#24262b;border:1px solid #33363d;border-radius:12px;padding:18px;margin:14px 0;display:none;white-space:pre-wrap}}.muted{{color:#9aa0aa;font-size:13px}}</style></head><body><h1>mneme 离线复习</h1><div class="muted" id="stat"></div><div id="q"></div><button onclick="show()">显示答案</button><button onclick="grade(1)">重来</button><button onclick="grade(3)">良好</button><button onclick="grade(4)">简单</button><div id="a"></div><div class="muted">复习记录保存在本机 localStorage，回连后用 <code>mneme sync</code> 回传。</div><script>const DATA={data};let i=0;const KEY='mneme-offline-events';function render(){{if(!DATA.cards.length){{document.getElementById('q').textContent='没有卡片';return}}const c=DATA.cards[i%DATA.cards.length];document.getElementById('q').textContent=c.prompt;document.getElementById('a').textContent=c.answer;document.getElementById('a').style.display='none';document.getElementById('stat').textContent=(i+1)+' / '+DATA.cards.length+' · '+DATA.entities.length+' 条记录'}}function show(){{document.getElementById('a').style.display='block'}}function grade(g){{const c=DATA.cards[i%DATA.cards.length];const ev={{card_id:c.id,ts:Date.now(),grade:g,device:'offline',seq:0}};const a=JSON.parse(localStorage.getItem(KEY)||'[]');a.push(ev);localStorage.setItem(KEY,JSON.stringify(a));i++;render()}}render()</script></body></html>"#
    )
    .into_bytes()
}
