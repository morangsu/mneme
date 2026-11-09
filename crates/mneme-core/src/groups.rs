//! 多级分组：把一层 `group_name` 字符串升级成树。
//!
//! 为什么要做层级：中药学天然是「章 → 节 → 药群」三层（解表药 → 发散风寒药 → 麻黄），
//! 一层字符串分组只能表达最末一级，用户看不出「这味药属于哪一章」。
//!
//! 为什么 `Group.parent` 而不是 `path` 字符串：改一级名字要重写所有子孙的路径，
//! 而 parent 只需改一处；代价是读的时候要做一次树遍历 —— 分组数量很小，可以忽略。

use crate::entity::Group;
use std::collections::BTreeMap;

/// 层级上限。超过 5 层人眼已经很难在手机上读，且多半是建模出了问题。
pub const MAX_DEPTH: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct GroupNode {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    /// 深度，顶层 = 0
    pub depth: usize,
    /// 从根到本节点的**名称**路径（显示用）：`["解表药", "发散风寒药"]`
    pub path_names: Vec<String>,
    /// 从根到本节点的 id 路径（判定与移动用）
    pub path_ids: Vec<String>,
}

impl GroupNode {
    /// "解表药 / 发散风寒药"
    pub fn display_path(&self) -> String {
        self.path_names.join(" / ")
    }
}

/// 把扁平的分组表整理成树（深度优先、同级按名称排序）。
///
/// 容错：父节点不存在时把该节点当顶层 —— 分组数据来自同步与导入，
/// 有可能拿到「指向已删除父级」的旧记录，此时宁可多显示一层，也不要整棵树消失。
pub fn tree(groups: &[Group]) -> Vec<GroupNode> {
    let alive: BTreeMap<&str, &Group> = groups
        .iter()
        .filter(|g| !g.deleted)
        .map(|g| (g.id.as_str(), g))
        .collect();

    // children: parent_id(None=root) -> 子节点 id
    let mut children: BTreeMap<Option<&str>, Vec<&str>> = BTreeMap::new();
    for g in alive.values() {
        let parent = g
            .parent
            .as_deref()
            .filter(|p| alive.contains_key(p))
            .filter(|p| *p != g.id);
        children.entry(parent).or_default().push(g.id.as_str());
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| {
            let na = alive.get(*a).map(|g| g.name.as_str()).unwrap_or("");
            let nb = alive.get(*b).map(|g| g.name.as_str()).unwrap_or("");
            na.cmp(nb).then(a.cmp(b))
        });
    }

    let mut out: Vec<GroupNode> = Vec::with_capacity(alive.len());
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    // 显式栈做深搜，避免递归爆栈（分组数量本来就不大，但不想留这种隐患）
    let mut stack: Vec<(&str, Option<&str>, Vec<String>, Vec<String>)> = Vec::new();
    if let Some(roots) = children.get(&None) {
        for id in roots.iter().rev() {
            stack.push((id, None, Vec::new(), Vec::new()));
        }
    }
    while let Some((id, _parent, mut names, mut ids)) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(g) = alive.get(id) else { continue };
        names.push(g.name.clone());
        ids.push(g.id.clone());
        out.push(GroupNode {
            id: g.id.clone(),
            name: g.name.clone(),
            parent: g.parent.clone(),
            depth: ids.len() - 1,
            path_names: names.clone(),
            path_ids: ids.clone(),
        });
        if ids.len() > MAX_DEPTH {
            // 超过上限不再往下展开，防止脏数据把界面撑爆
            continue;
        }
        if let Some(kids) = children.get(&Some(id)) {
            for kid in kids.iter().rev() {
                stack.push((kid, Some(id), names.clone(), ids.clone()));
            }
        }
    }
    out
}

/// 某个分组的名称路径；分组不存在时返回空。
pub fn path_names(groups: &[Group], id: &str) -> Vec<String> {
    tree(groups)
        .into_iter()
        .find(|n| n.id == id)
        .map(|n| n.path_names)
        .unwrap_or_default()
}

/// 把 `candidate` 设为 `id` 的父级是否会成环（含自己指向自己）。
pub fn would_cycle(groups: &[Group], id: &str, candidate_parent: Option<&str>) -> bool {
    let Some(parent) = candidate_parent else {
        return false;
    };
    if parent == id {
        return true;
    }
    let by_id: BTreeMap<&str, &Group> = groups.iter().map(|g| (g.id.as_str(), g)).collect();
    let mut cur = Some(parent);
    let mut hops = 0;
    while let Some(c) = cur {
        if c == id {
            return true;
        }
        hops += 1;
        if hops > MAX_DEPTH + 1 {
            // 数据本身已经有环，别再转下去
            return true;
        }
        cur = by_id.get(c).and_then(|g| g.parent.as_deref());
    }
    false
}

/// 直接子分组数量（删除时要提示会牵连谁）。
pub fn child_count(groups: &[Group], id: &str) -> usize {
    groups.iter().filter(|g| !g.deleted && g.parent.as_deref() == Some(id)).count()
}
