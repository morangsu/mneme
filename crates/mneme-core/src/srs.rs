use crate::entity::ReviewEvent;
use std::collections::{HashMap, HashSet};

const DAY_MS: i64 = 86_400_000;

/// 复习状态。**不跨设备同步**，只由事件重算（见 `fold_events`）。
#[derive(Debug, Clone, PartialEq)]
pub struct SrsState {
    pub stability: f64,
    pub difficulty: f64,
    pub due: i64,
    pub reps: u32,
    pub lapses: u32,
    pub last: i64,
}

impl Default for SrsState {
    fn default() -> Self {
        Self {
            stability: 1.0,
            difficulty: 5.0,
            due: 0,
            reps: 0,
            lapses: 0,
            last: 0,
        }
    }
}

/// 可替换的排期器。想换完整 FSRS 只需实现这个 trait，同步层与存储层不动。
pub trait Scheduler: Send + Sync {
    fn schedule(&self, prev: &SrsState, grade: u8, now_ms: i64) -> SrsState;
}

/// 简化 SM-2 + stability 概念：足够做地基，且完全确定性（便于回放测试）。
#[derive(Debug, Clone, Copy)]
pub struct Sm2Scheduler {
    pub max_interval_days: f64,
}

impl Default for Sm2Scheduler {
    fn default() -> Self {
        Self {
            max_interval_days: 365.0,
        }
    }
}

impl Scheduler for Sm2Scheduler {
    fn schedule(&self, prev: &SrsState, grade: u8, now_ms: i64) -> SrsState {
        let g = grade.clamp(1, 4);
        if g < 3 {
            return SrsState {
                stability: (prev.stability * 0.5).max(0.5),
                difficulty: (prev.difficulty + 0.2).min(10.0),
                due: now_ms + DAY_MS,
                reps: prev.reps,
                lapses: prev.lapses + 1,
                last: now_ms,
            };
        }
        let factor = (1.2 + 0.35 * (g as f64 - 2.0) - 0.05 * (prev.difficulty - 5.0)).max(1.05);
        let stability = (prev.stability * factor).min(self.max_interval_days);
        let days = stability.round().max(1.0) as i64;
        SrsState {
            stability,
            difficulty: (prev.difficulty - 0.1 + 0.05 * (4 - g) as f64).max(1.0),
            due: now_ms + days * DAY_MS,
            reps: prev.reps + 1,
            lapses: prev.lapses,
            last: now_ms,
        }
    }
}

/// 从 append-only 事件重放出全部卡片状态。
/// 以事件自身的 `ts` 作为时间基准 -> 任何设备、任何顺序重放结果一致。
pub fn fold_events<S: Scheduler + ?Sized>(
    events: &[ReviewEvent],
    scheduler: &S,
) -> HashMap<String, SrsState> {
    let mut seen = HashSet::with_capacity(events.len());
    let mut sorted: Vec<&ReviewEvent> = events
        .iter()
        .filter(|e| seen.insert(e.key()))
        .collect();
    sorted.sort_by(|a, b| {
        a.ts.cmp(&b.ts)
            .then(a.seq.cmp(&b.seq))
            .then(a.device.cmp(&b.device))
            .then(a.card_id.cmp(&b.card_id))
    });
    let mut states: HashMap<String, SrsState> = HashMap::with_capacity(events.len());
    for e in sorted {
        let cur = states.entry(e.card_id.clone()).or_default();
        let next = scheduler.schedule(cur, e.grade, e.ts);
        *cur = next;
    }
    states
}

pub fn due_cards(states: &HashMap<String, SrsState>, now: i64, limit: usize) -> Vec<String> {
    let mut v: Vec<(&String, &SrsState)> = states
        .iter()
        .filter(|(_, s)| s.due <= now)
        .collect();
    v.sort_by(|a, b| a.1.due.cmp(&b.1.due).then(a.0.cmp(b.0)));
    v.into_iter()
        .take(limit)
        .map(|(id, _)| id.clone())
        .collect()
}

/// 每日队列：到期卡优先（越逾期越靠前），新卡限量、按实体聚簇。
/// 纯函数 —— 服务端 / 桌面端 / FFI 三处共用同一份判定。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DailyQueue {
    /// 到期卡（已复习过、due <= now）
    pub due: Vec<String>,
    /// 新卡（从未复习过）
    pub new: Vec<String>,
    /// 会话顺序 = due 在前，new 在后
    pub items: Vec<String>,
}

pub fn daily_queue(
    cards: &[crate::card::Card],
    states: &HashMap<String, SrsState>,
    now: i64,
    new_limit: usize,
    due_limit: usize,
) -> DailyQueue {
    let mut due: Vec<(&str, i64)> = cards
        .iter()
        .filter_map(|c| states.get(&c.id).filter(|s| s.due <= now).map(|s| (c.id.as_str(), s.due)))
        .collect();
    due.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(b.0)));
    let due: Vec<String> = due.into_iter().take(due_limit).map(|(id, _)| id.to_string()).collect();

    // 新卡按 (实体, 方向, id) 排序：同一味药的几张卡会连着出现，认知上更连贯
    let mut fresh: Vec<&crate::card::Card> =
        cards.iter().filter(|c| !states.contains_key(&c.id)).collect();
    fresh.sort_by(|a, b| {
        a.entity_id
            .cmp(&b.entity_id)
            .then(a.direction.cmp(&b.direction))
            .then(a.id.cmp(&b.id))
    });
    let new: Vec<String> = fresh.iter().take(new_limit).map(|c| c.id.clone()).collect();

    let mut items = due.clone();
    items.extend(new.clone());
    DailyQueue { due, new, items }
}

/// 自动评分建议：判分对错 + 相似度 + 用时 → 1(重来)/2(困难)/3(良好)/4(简单)。
pub fn suggest_grade(correct: bool, elapsed_ms: i64, score: f64) -> (u8, &'static str) {
    if !correct {
        return (1, "答错，重来");
    }
    if score < 0.95 {
        return (2, "接近但不够准");
    }
    if elapsed_ms > 15_000 {
        return (2, "想太久");
    }
    if elapsed_ms >= 5_000 {
        return (3, "正常速度");
    }
    (4, "脱口而出")
}

/// 错题（薄弱）卡：最近 30 天**末尾连续答错** >=2 次。
/// 连续答对会清零末尾 streak，自然移出错题本 —— 不需要单独的移出逻辑。
#[derive(Debug, Clone, PartialEq)]
pub struct WeakCard {
    pub card_id: String,
    pub misses: u32,
    pub last_ts: i64,
}

pub fn weak_cards(events: &[ReviewEvent], now: i64, limit: usize) -> Vec<WeakCard> {
    let cutoff = now - 30 * DAY_MS;
    let mut by_card: HashMap<&str, Vec<&ReviewEvent>> = HashMap::new();
    for e in events {
        if e.ts >= cutoff {
            by_card.entry(e.card_id.as_str()).or_default().push(e);
        }
    }
    let mut out: Vec<WeakCard> = Vec::new();
    for (id, evs) in by_card {
        let mut sorted = evs;
        sorted.sort_by_key(|e| e.ts);
        let mut streak = 0u32;
        for e in sorted.iter().rev() {
            if e.grade == 1 {
                streak += 1;
            } else {
                break;
            }
        }
        if streak >= 2 {
            out.push(WeakCard {
                card_id: id.to_string(),
                misses: sorted.iter().filter(|e| e.grade == 1).count() as u32,
                last_ts: sorted.last().map(|e| e.ts).unwrap_or(0),
            });
        }
    }
    out.sort_by(|a, b| {
        b.misses
            .cmp(&a.misses)
            .then(b.last_ts.cmp(&a.last_ts))
            .then(a.card_id.cmp(&b.card_id))
    });
    out.truncate(limit);
    out
}

/// 掌握度派生：写回实体 `mastery` 字段的三档标签。
/// stability 取该实体已复习卡片的均值，lapses 求和，last_grade 取最新一条事件的评分。
pub fn mastery_label(stability: f64, lapses: u32, last_grade: Option<u8>) -> &'static str {
    if lapses >= 2 || stability < 1.0 {
        "生疏"
    } else if stability < 7.0 {
        "熟悉"
    } else if last_grade.map(|g| g >= 3).unwrap_or(false) {
        "熟练"
    } else {
        "熟悉"
    }
}

/// FSRS 风格调度器（DSR：Difficulty / Stability / Retrievability）。
/// 与 `Sm2Scheduler` 实现同一个 `Scheduler` trait，可随时替换；
/// 参数可由用户自己的复习记录拟合（`fit`），不依赖外部服务。
#[derive(Debug, Clone)]
pub struct FsrsScheduler {
    pub request_retention: f64,
    /// [初始稳定性×4, 难度系数, 稳定性增长, 遗忘曲线因子, 难度权重, 失败惩罚, 上限, 曲线指数]
    pub w: [f64; 9],
}

impl Default for FsrsScheduler {
    fn default() -> Self {
        Self {
            request_retention: 0.9,
            w: [1.2, 2.0, 3.2, 5.0, 0.5, 2.4, 0.9, 0.7, 1.0],
        }
    }
}

impl FsrsScheduler {
    /// 幂律遗忘曲线：R(t) = (1 + c·t/S)^-1
    pub fn retrievability(&self, elapsed_days: f64, stability: f64) -> f64 {
        (1.0 + self.w[8] * elapsed_days.max(0.0) / stability.max(0.1)).recip()
    }

    fn initial_stability(&self, grade: u8) -> f64 {
        self.w[(grade.clamp(1, 4) - 1) as usize]
    }

    fn difficulty(&self, prev: f64, grade: u8) -> f64 {
        let delta = (4 - grade.clamp(1, 4)) as f64 - 2.0;
        (prev + self.w[4] * delta).clamp(1.0, 10.0)
    }

    fn next_stability(&self, s: f64, d: f64, r: f64, grade: u8) -> f64 {
        let g = grade.clamp(1, 4);
        if g < 3 {
            return (s * self.w[7]).max(0.5);
        }
        let boost = 1.0
            + self.w[5]
                * (11.0 - d)
                * s.powf(-0.3)
                * ((1.0 - r).max(0.01) + 0.05 * (g as f64 - 2.0));
        (s * boost).min(self.w[6] * 365.0)
    }

    /// 用个人复习记录做网格搜索，挑预测误差最小的目标保持率。
    /// 完全本地、确定性，不上传任何数据。
    pub fn fit(&mut self, events: &[ReviewEvent]) -> f64 {
        let mut sorted: Vec<&ReviewEvent> = events.iter().collect();
        sorted.sort_by_key(|e| e.ts);
        let mut best = (0.9f64, f64::MAX);
        for rr in [0.75, 0.8, 0.85, 0.9, 0.95] {
            let mut trial = self.clone();
            trial.request_retention = rr;
            let mut state = SrsState::default();
            let mut err = 0.0f64;
            let mut n = 0usize;
            let mut prev_ts: Option<i64> = None;
            for e in &sorted {
                if let Some(pt) = prev_ts {
                    let days = (e.ts - pt).max(0) as f64 / 86_400_000.0;
                    let predicted = trial.retrievability(days, state.stability);
                    let actual = if e.grade >= 3 { 1.0 } else { 0.0 };
                    err += (predicted - actual).powi(2);
                    n += 1;
                }
                state = trial.schedule(&state, e.grade, e.ts);
                prev_ts = Some(e.ts);
            }
            let score = if n > 0 { err / n as f64 } else { f64::MAX };
            if score < best.1 {
                best = (rr, score);
            }
        }
        self.request_retention = best.0;
        best.0
    }
}

impl Scheduler for FsrsScheduler {
    fn schedule(&self, prev: &SrsState, grade: u8, now_ms: i64) -> SrsState {
        let g = grade.clamp(1, 4);
        let elapsed = if prev.last > 0 {
            (now_ms - prev.last).max(0) as f64 / 86_400_000.0
        } else {
            0.0
        };
        let s0 = if prev.reps == 0 && prev.lapses == 0 {
            self.initial_stability(g)
        } else {
            prev.stability
        };
        let r = self.retrievability(elapsed, s0);
        let d = self.difficulty(prev.difficulty, g);
        let s = self.next_stability(s0, d, r, g);
        // 反解 R(t) = (1 + c·t/S)^-1 = rr  =>  t = S·(1/rr - 1)/c
        let days = (s * (1.0 / self.request_retention.clamp(0.5, 0.99) - 1.0) / self.w[8])
            .max(1.0)
            .round() as i64;
        let days = days.clamp(1, 3650);
        SrsState {
            stability: s,
            difficulty: d,
            due: now_ms + days * DAY_MS,
            reps: if g >= 3 { prev.reps + 1 } else { prev.reps },
            lapses: if g < 3 { prev.lapses + 1 } else { prev.lapses },
            last: now_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsrs_higher_grade_gives_longer_interval() {
        let f = FsrsScheduler::default();
        let a = f.schedule(&SrsState::default(), 3, 1000);
        let b = f.schedule(&SrsState::default(), 4, 1000);
        assert!(b.stability >= a.stability);
        assert!(b.due >= a.due);
        let bad = f.schedule(&b, 1, 2000);
        assert!(bad.lapses == 1);
        assert!(bad.stability < b.stability);
    }

    #[test]
    fn fsrs_fit_is_deterministic_and_bounded() {
        let mut f = FsrsScheduler::default();
        let ev: Vec<ReviewEvent> = (0..12)
            .map(|i| ReviewEvent {
                card_id: "c".into(),
                ts: 1000 + i * 86_400_000,
                grade: if i % 3 == 0 { 2 } else { 4 },
                device: "w".into(),
                seq: 0,
            })
            .collect();
        let r = f.fit(&ev);
        assert!((0.75..=0.95).contains(&r));
        assert_eq!(f.fit(&ev), r);
    }

    fn ev(card: &str, ts: i64, grade: u8, device: &str) -> ReviewEvent {
        ReviewEvent {
            card_id: card.into(),
            ts,
            grade,
            device: device.into(),
            seq: 0,
        }
    }

    #[test]
    fn replay_is_order_independent_and_idempotent() {
        let a = vec![ev("c1", 1000, 4, "win"), ev("c1", 2000, 3, "phone")];
        let b = vec![
            ev("c1", 1000, 4, "win"),
            ev("c1", 1000, 4, "win"), // 重复回传
            ev("c1", 2000, 3, "phone"),
        ];
        let s = Sm2Scheduler::default();
        let x = fold_events(&a, &s);
        let y = fold_events(&b, &s);
        assert_eq!(x, y);
        assert_eq!(x["c1"].reps, 2);
    }

    #[test]
    fn failure_resets_interval() {
        let s = Sm2Scheduler::default();
        let good = s.schedule(&SrsState::default(), 4, 1000);
        let bad = s.schedule(&good, 1, 2000);
        assert!(bad.lapses == 1);
        assert!(bad.due - 2000 <= DAY_MS);
    }

    #[test]
    fn due_cards_sorted() {
        let s = Sm2Scheduler::default();
        let events = vec![ev("c1", 1000, 4, "w"), ev("c2", 1000, 1, "w")];
        let states = fold_events(&events, &s);
        let due = due_cards(&states, 1000 + 2 * DAY_MS, 10);
        assert!(due.contains(&"c2".to_string()));
    }
}
