use crate::entity::ReviewEvent;
use serde::{Deserialize, Serialize};
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
    /// 学习步骤下标；等于 `learning_steps.len()` 表示已毕业进入复习态。
    pub step: u8,
    /// 累计遗忘超过阈值后的标记（建议改写这张卡，而不是继续硬背）。
    pub leech: bool,
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
            step: 0,
            leech: false,
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
                step: 0,
                leech: false,
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
            step: 0,
            leech: false,
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

/// 复习参数（用户可调，落到本地 `review.json`）。
///
/// 默认值取 Anki 常用配置与 FSRS 官方建议：
/// 新卡 10/天、到期上限 100/天、期望记忆率 0.9、学习步骤 1m+10m、leech 阈值 8。
/// 「本轮张数」只影响这一次会话拿多少张，不改每日配额。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    pub new_per_day: u32,
    pub reviews_per_day: u32,
    /// 单次会话最多拿多少张（0 = 不限，用每日配额）。
    pub session_limit: u32,
    pub desired_retention: f64,
    pub max_interval_days: f64,
    pub easy_interval_days: f64,
    pub leech_threshold: u32,
    /// 学习步骤（分钟）。
    #[serde(default = "default_learning_steps")]
    pub learning_steps_minutes: Vec<f64>,
}

fn default_learning_steps() -> Vec<f64> {
    vec![1.0, 10.0]
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            new_per_day: 10,
            reviews_per_day: 100,
            session_limit: 20,
            desired_retention: 0.9,
            max_interval_days: 3650.0,
            easy_interval_days: 4.0,
            leech_threshold: 8,
            learning_steps_minutes: default_learning_steps(),
        }
    }
}

impl ReviewConfig {
    /// 把越界值夹回合理区间：宁可悄悄纠正，也不要因为一个手滑把排期搞崩。
    pub fn normalized(mut self) -> Self {
        self.new_per_day = self.new_per_day.min(500);
        self.reviews_per_day = self.reviews_per_day.min(2000);
        self.session_limit = self.session_limit.min(500);
        self.desired_retention = self.desired_retention.clamp(0.70, 0.97);
        self.max_interval_days = self.max_interval_days.clamp(1.0, 36500.0);
        self.easy_interval_days = self.easy_interval_days.clamp(1.0, 60.0);
        self.leech_threshold = self.leech_threshold.clamp(2, 50);
        if self.learning_steps_minutes.is_empty() {
            self.learning_steps_minutes = default_learning_steps();
        }
        self.learning_steps_minutes = self
            .learning_steps_minutes
            .iter()
            .map(|m| m.clamp(0.5, 1440.0))
            .collect();
        self
    }

    pub fn scheduler(&self) -> FsrsScheduler {
        let mut f = FsrsScheduler::default();
        f.request_retention = self.desired_retention;
        f.max_interval_days = self.max_interval_days;
        f.easy_interval_days = self.easy_interval_days;
        f.leech_threshold = self.leech_threshold;
        f.learning_steps_ms = self
            .learning_steps_minutes
            .iter()
            .map(|m| (*m * 60_000.0).round() as i64)
            .collect();
        f.relearn_steps_ms = vec![f.learning_steps_ms[0].max(600_000)];
        f
    }

    /// 重学步骤取学习步骤的第一档——比固定 10 分钟更符合「忘记后回到起点」。
    pub fn relearn_step_ms(&self) -> i64 {
        self.scheduler().relearn_steps_ms[0]
    }
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

/// 掌握度派生（带覆盖率）。
///
/// 为什么需要 `reviewed / total`：一味药的十几张卡里只要有一张刚加入，
/// 纯均值就会被它的低稳定度稀释 —— 实测「复习一张新卡，熟练掉成熟悉」，
/// 用户看到的是「我答得好反而降级了」。
///
/// 规则（按优先级）：
/// 1. 有两次以上遗忘，或平均稳定度 < 1 天 → **生疏**
/// 2. 平均稳定度 < 7 天 → **熟悉**
/// 3. 覆盖率 < 80% → **熟悉**（还有没学过的角度，先别自称熟练）
/// 4. 最近一次评分 >= 良好 → **熟练**，否则 **熟悉**
pub fn mastery_label_ext(
    stability_avg: f64,
    lapses: u32,
    last_grade: Option<u8>,
    reviewed: u32,
    total: u32,
) -> &'static str {
    if lapses >= 2 || stability_avg < 1.0 {
        return "生疏";
    }
    if stability_avg < 7.0 {
        return "熟悉";
    }
    // 覆盖率：所有卡都没复习过时 total 可能为 0，按已学完处理
    if total > 0 && (reviewed as f64) / (total as f64) < 0.8 {
        return "熟悉";
    }
    if last_grade.map(|g| g >= 3).unwrap_or(false) {
        "熟练"
    } else {
        "熟悉"
    }
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

/// FSRS-5（Free Spaced Repetition Scheduler，open-spaced-repetition 项目）实现，
/// 外加 Anki 式的学习步骤与 leech 判定。
///
/// 与简化 SM-2 的差别：记忆状态显式建模为 DSR ——
/// - `S`（stability）稳定度：回忆概率降到 90% 所需的天数；
/// - `D`（difficulty）难度 1–10；
/// - `R`（retrievability）当前可回忆概率。
///
/// 遗忘曲线：`R(t,S) = (1 + FACTOR·t/S)^DECAY`，`DECAY=-0.5`、`FACTOR=19/81`，
/// 取 `t=S` 时 `R=0.9`（这正是 stability 的定义）。
/// 反解得到间隔：`I(r,S) = S/FACTOR · (r^(1/DECAY) − 1)` ——
/// 间隔由「期望记忆率」决定，而不是乘一个拍脑袋的系数。
///
/// 参考资料：
/// - FSRS-5 算法与默认参数：github.com/open-spaced-repetition/fsrs4anki/wiki/The-Algorithm
/// - Anki 牌组选项（学习步骤 / leech 阈值 / 最大间隔）：docs.ankiweb.net/deck-options.html
/// - 85% 规则（检索难度与学习效率的最优区间）：Wilson et al. 2019, Nature Communications
/// - 测试效应 / 间隔效应 / 合意困难：Roediger & Karpicke 2006；Bjork 2011
#[derive(Debug, Clone)]
pub struct FsrsScheduler {
    /// 期望记忆率：0.9 表示「复习时约有 90% 概率还记得」。
    /// 调低 → 间隔更长、复习更少、遗忘更多。研究建议 0.85–0.9。
    pub request_retention: f64,
    /// FSRS-5 的 19 个权重（官方默认值，可直接用；将来可用 `fit` 在本地调优）。
    pub w: [f64; 19],
    pub max_interval_days: f64,
    /// 新卡第一次就按「简单」时给的固定间隔（Anki 的 easy interval，默认 4 天）。
    pub easy_interval_days: f64,
    /// 学习步骤（毫秒）：新卡先在这些短间隔里过一遍，毕业才进天级复习。
    pub learning_steps_ms: Vec<i64>,
    /// 遗忘后的重学步骤（毫秒）。
    pub relearn_steps_ms: Vec<i64>,
    /// 累计遗忘达到多少次判为 leech。
    pub leech_threshold: u32,
}

/// FSRS-5 官方默认参数。
pub const FSRS5_DEFAULT_W: [f64; 19] = [
    0.4072, 1.1829, 3.1262, 15.4722, 7.2102, 0.5316, 1.0651, 0.0234, 1.616, 0.1544, 1.0824,
    1.9813, 0.0953, 0.2975, 2.2042, 0.2407, 2.9466, 0.5034, 0.6567,
];

const DECAY: f64 = -0.5;
const FACTOR: f64 = 19.0 / 81.0;

impl Default for FsrsScheduler {
    fn default() -> Self {
        Self {
            request_retention: 0.9,
            w: FSRS5_DEFAULT_W,
            max_interval_days: 3650.0,
            easy_interval_days: 4.0,
            learning_steps_ms: vec![60_000, 600_000],
            relearn_steps_ms: vec![600_000],
            leech_threshold: 8,
        }
    }
}

impl FsrsScheduler {
    /// 幂律遗忘曲线 R(t,S)。t 与 S 都以「天」为单位。
    pub fn retrievability(&self, elapsed_days: f64, stability: f64) -> f64 {
        (1.0 + FACTOR * elapsed_days.max(0.0) / stability.max(0.01)).powf(DECAY)
    }

    /// 反解：让可回忆概率降到 `retention` 需要多少天。
    pub fn interval_days(&self, stability: f64, retention: f64) -> f64 {
        let r = retention.clamp(0.5, 0.99);
        (stability / FACTOR * (r.powf(1.0 / DECAY) - 1.0)).max(0.0)
    }

    fn initial_stability(&self, grade: u8) -> f64 {
        self.w[(grade.clamp(1, 4) - 1) as usize].max(0.01)
    }

    /// D0(G) = w4 − e^(w5·(G−1)) + 1
    fn initial_difficulty(&self, grade: u8) -> f64 {
        (self.w[4] - (self.w[5] * (grade.clamp(1, 4) as f64 - 1.0)).exp() + 1.0)
            .clamp(1.0, 10.0)
    }

    /// D′ = D − w6·(G−3)，再向 D0(4) 做均值回归（防止「难度地狱」）。
    fn next_difficulty(&self, prev: f64, grade: u8) -> f64 {
        let delta = -self.w[6] * (grade.clamp(1, 4) as f64 - 3.0);
        let d = prev + delta;
        let target = self.initial_difficulty(4);
        (self.w[7] * target + (1.0 - self.w[7]) * d).clamp(1.0, 10.0)
    }

    /// 成功回忆后的新 stability。
    fn recall_stability(&self, s: f64, d: f64, r: f64, grade: u8) -> f64 {
        let g = grade.clamp(1, 4);
        let hard_penalty = if g == 2 { self.w[15] } else { 1.0 };
        let easy_bonus = if g == 4 { self.w[16] } else { 1.0 };
        let gain = self.w[8].exp()
            * (11.0 - d)
            * s.powf(-self.w[9])
            * ((self.w[10] * (1.0 - r)).exp() - 1.0)
            * hard_penalty
            * easy_bonus;
        (s * (1.0 + gain)).max(0.01)
    }

    /// 遗忘后的新 stability（不超过原值）。
    fn forget_stability(&self, s: f64, d: f64, r: f64) -> f64 {
        let sf = self.w[11]
            * d.powf(-self.w[12])
            * ((s + 1.0).powf(self.w[13]) - 1.0)
            * (self.w[14] * (1.0 - r)).exp();
        sf.min(s).max(0.01)
    }

    /// 同日复习的短时 stability（FSRS-5）：S′ = S·e^(w17·(G−3+w18))
    fn short_term_stability(&self, s: f64, grade: u8) -> f64 {
        let g = grade.clamp(1, 4) as f64;
        (s * (self.w[17] * (g - 3.0 + self.w[18])).exp()).max(0.01)
    }

    /// 按 stability 与期望记忆率算出间隔（天），并夹到 [1, max]。
    fn interval_from(&self, stability: f64) -> i64 {
        let days = self.interval_days(stability, self.request_retention).round();
        days.clamp(1.0, self.max_interval_days.max(1.0)) as i64
    }

    /// 用个人复习记录做网格搜索，挑预测误差最小的期望记忆率。
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
                    let days = (e.ts - pt).max(0) as f64 / DAY_MS as f64;
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
        let mut state = prev.clone();
        state.reps += 1;
        state.last = now_ms;
        let steps = self.learning_steps_ms.len();

        // ---- 首次复习：只定初始 S/D，并按学习步骤排下一次 ----
        if prev.reps == 0 {
            state.stability = self.initial_stability(g);
            state.difficulty = self.initial_difficulty(g);
            match g {
                1 => {
                    state.step = 0;
                    state.due = now_ms + self.learning_steps_ms.first().copied().unwrap_or(DAY_MS);
                }
                2 => {
                    state.step = 0;
                    state.due = now_ms + self.learning_steps_ms.first().copied().unwrap_or(DAY_MS);
                }
                3 if steps > 1 => {
                    state.step = 1;
                    state.due = now_ms + self.learning_steps_ms[1];
                }
                3 => {
                    // 没有学习步骤：直接按 stability 给天级间隔
                    state.step = steps as u8;
                    state.due = now_ms + self.interval_from(state.stability) * DAY_MS;
                }
                _ => {
                    state.step = steps as u8;
                    let days = self.easy_interval_days.max(1.0) as i64;
                    state.due = now_ms + days * DAY_MS;
                }
            }
            state.leech = false;
            return state;
        }

        // ---- 学习 / 重学阶段：按步骤推进，不走天级公式 ----
        if (prev.step as usize) < steps {
            let cur = prev.step as usize;
            match g {
                1 => {
                    state.lapses += 1;
                    state.step = 0;
                    state.due = now_ms + self.learning_steps_ms[0];
                }
                2 => {
                    state.due = now_ms + self.learning_steps_ms[cur.min(steps - 1)];
                }
                _ => {
                    if cur + 1 < steps {
                        state.step = (cur + 1) as u8;
                        state.due = now_ms + self.learning_steps_ms[cur + 1];
                    } else {
                        // 毕业：按 stability 与期望记忆率进入天级间隔
                        state.step = steps as u8;
                        state.due = now_ms + self.interval_from(state.stability) * DAY_MS;
                    }
                }
            }
            state.leech = state.lapses >= self.leech_threshold;
            return state;
        }

        // ---- 复习态：DSR 更新 ----
        let elapsed_days = (now_ms - prev.last).max(0) as f64 / DAY_MS as f64;
        let r = self.retrievability(elapsed_days, prev.stability);
        state.difficulty = self.next_difficulty(prev.difficulty, g);

        if g == 1 {
            state.lapses += 1;
            state.stability = if elapsed_days < 1.0 {
                self.short_term_stability(prev.stability, 1)
            } else {
                self.forget_stability(prev.stability, state.difficulty, r)
            };
            state.step = 0;
            let relearn = self.relearn_steps_ms.first().copied().unwrap_or(600_000);
            state.due = now_ms + relearn;
        } else {
            state.stability = if elapsed_days < 1.0 {
                self.short_term_stability(prev.stability, g)
            } else {
                self.recall_stability(prev.stability, state.difficulty, r, g)
            };
            state.due = now_ms + self.interval_from(state.stability) * DAY_MS;
        }
        state.leech = state.lapses >= self.leech_threshold;
        state
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
    fn fsrs_interval_formula_inverts_at_90_percent() {
        let f = FsrsScheduler::default();
        // stability 的定义：R 降到 0.9 的那一天
        assert!((f.interval_days(30.0, 0.9) - 30.0).abs() < 0.01);
        assert!(f.retrievability(30.0, 30.0) > 0.89 && f.retrievability(30.0, 30.0) < 0.91);
        // 期望记忆率越高，间隔越短
        assert!(f.interval_days(30.0, 0.95) < f.interval_days(30.0, 0.8));
    }

    #[test]
    fn fsrs_new_card_goes_through_learning_steps() {
        let f = FsrsScheduler::default();
        let now = 1_000_000;
        let a = f.schedule(&SrsState::default(), 3, now);
        // 第一步 10 分钟，还没进天级
        assert_eq!(a.step, 1);
        assert_eq!(a.due, now + 600_000);
        // 再来一次「良好」毕业，间隔按 stability 与期望记忆率算
        let b = f.schedule(&a, 3, a.due);
        assert_eq!(b.step, 2, "两步学习结束，进入复习态");
        assert!(b.due - a.due >= DAY_MS, "毕业后至少一天");
        // 简单卡直接跳过学习步骤
        let easy = f.schedule(&SrsState::default(), 4, now);
        assert_eq!(easy.step, 2);
        assert_eq!(easy.due, now + 4 * DAY_MS, "easy interval 默认 4 天");
    }

    #[test]
    fn fsrs_hard_shorter_good_mid_easy_long() {
        let f = FsrsScheduler::default();
        let now = 10 * DAY_MS;
        let mut base = SrsState::default();
        base.reps = 5;
        base.step = 2;
        base.stability = 20.0;
        base.difficulty = 5.0;
        base.last = now - 20 * DAY_MS;
        let hard = f.schedule(&base, 2, now);
        let good = f.schedule(&base, 3, now);
        let easy = f.schedule(&base, 4, now);
        assert!(hard.due <= good.due, "困难不应比良好还长");
        assert!(good.due <= easy.due, "良好不应比简单还长");
        assert!(good.stability > base.stability, "答对后稳定度应增长");
    }

    #[test]
    fn fsrs_failure_shrinks_stability_and_schedules_relearn() {
        let f = FsrsScheduler::default();
        let now = 10 * DAY_MS;
        let mut base = SrsState::default();
        base.reps = 3;
        base.step = 2;
        base.stability = 30.0;
        base.difficulty = 4.0;
        base.last = now - 30 * DAY_MS;
        let after = f.schedule(&base, 1, now);
        assert_eq!(after.lapses, 1);
        assert!(after.stability < base.stability);
        assert_eq!(after.step, 0, "回到重学步骤");
        assert_eq!(after.due, now + 600_000, "重学步骤默认 10 分钟");
    }

    #[test]
    fn fsrs_retention_controls_interval_length() {
        let mut f = FsrsScheduler::default();
        let now = 10 * DAY_MS;
        let mut base = SrsState::default();
        base.reps = 4;
        base.step = 2;
        base.stability = 20.0;
        base.difficulty = 5.0;
        base.last = now - 20 * DAY_MS;
        f.request_retention = 0.8;
        let loose = f.schedule(&base, 3, now);
        f.request_retention = 0.95;
        let tight = f.schedule(&base, 3, now);
        assert!(loose.due > tight.due, "低期望记忆率 → 更长间隔");
    }

    #[test]
    fn fsrs_marks_leech_after_threshold_lapses() {
        let f = FsrsScheduler::default();
        let mut st = SrsState::default();
        st.reps = 3;
        st.step = 2;
        st.stability = 5.0;
        st.difficulty = 6.0;
        let mut now = 10 * DAY_MS;
        for i in 0..8 {
            st.last = now - 5 * DAY_MS;
            st = f.schedule(&st, 1, now);
            now += 600_000;
            if i < 7 {
                assert!(!st.leech, "第 {} 次遗忘还不该判 leech", i + 1);
            }
        }
        assert_eq!(st.lapses, 8);
        assert!(st.leech, "累计 8 次遗忘应判 leech");
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
