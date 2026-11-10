// mneme 桌面前端：三栏 + 键盘优先。
//
// 设计约束（与现有网页端不同）：桌面是「键盘 + 多列 + 双栏详情」，不是单列触控。
// 但数据层完全复用 /api/*，检索与详情的口径都在 Rust 侧（search / detail_view），
// 这里只做渲染与键位绑定 —— 两端不写两份业务规则。
//
// 键位：/ 检索 · j/k 或 ↑↓ 移动 · Enter 详情 · [ ] 切视图 · Esc 收起

const TOKEN = new URLSearchParams(location.search).get('token') || '';
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

async function api(path, opt = {}) {
  const sep = path.includes('?') ? '&' : '?';
  const r = await fetch(path + sep + 'token=' + encodeURIComponent(TOKEN), opt);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return { error: t }; }
}

// ---- 全局状态 ----------------------------------------------------------
const S = {
  views: [],        // [{id,label}]
  viewId: null,
  view: null,       // /api/view/{id} 的返回
  mode: 'view',     // 'view' | 'search'
  hits: [],         // 检索结果
  items: [],        // 当前列表的行（统一抽象：{id,title,cells?}）
  sel: -1,          // 选中行下标
  query: '',
  viewMode: 'table',    // 'table' | 'cards'，localStorage 记忆
  groupMode: false,     // 按分组树聚合（?group=1）
  collapsed: new Set(), // 折叠的分组 key（路径 '>' 连接）
};
try { S.viewMode = localStorage.getItem('mneme.viewMode') === 'cards' ? 'cards' : 'table'; } catch (_) {}

// ---- 领域包引导：库还没有 schema 时显示入口，选 .toml 一键加载 ----------
// 与手机端首启 setSchema 同口径；加载成功后重新 boot 刷新视图。
$('domain-init').addEventListener('click', () => $('domain-file').click());
$('domain-file').addEventListener('change', async (e) => {
  const f = e.target.files[0];
  if (!f) return;
  $('search-note').textContent = `加载 ${f.name} …`;
  const toml = await f.text();
  const r = await api('/api/schema/init', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ toml }),
  });
  e.target.value = '';
  if (r.error) {
    $('search-note').textContent = '加载失败：' + r.error;
    return;
  }
  $('search-note').textContent =
    `领域 ${r.domain} 已加载：${r.fields} 字段 / ${r.directions} 出题方向 / ${r.views} 视图 / ${r.aliases} 同义`;
  await boot();
});

// ---- 启动 -------------------------------------------------------------
async function boot() {
  // 深链：&group=1 按分组聚合、&mode=cards 卡片模式（配合无头验证与分享）
  const qp0 = new URLSearchParams(location.search);
  if (qp0.get('group') === '1') S.groupMode = true;
  if (qp0.get('mode') === 'cards') S.viewMode = 'cards';
  const s = await api('/api/state');
  if (s.error) { $('list').innerHTML = `<div class="empty">${esc(s.error)}</div>`; return; }
  $('state').textContent = ` · ${s.schema || '未加载领域'} · ${s.entities} 条 · ${s.cards} 张卡 · rev ${s.rev}`;
  const v = await api('/api/views');
  S.views = v.views || [];
  $('views').innerHTML = S.views.map((x, i) =>
    `<div class="view-item" data-i="${i}">${esc(x.label || x.id)}</div>`).join('');
  document.querySelectorAll('.view-item').forEach((el) => {
    el.addEventListener('click', () => selectView(+el.dataset.i));
  });
  $('meta').textContent = `${s.entities} 条 · ${s.cards} 张卡 · 事件 ${s.events}`;
  // 没有 schema 就亮出「加载领域包」入口；加载成功重新 boot 后自动隐藏
  $('domain-init').hidden = !!s.schema;
  if (S.views.length) selectView(0);
}

async function selectView(i) {
  S.mode = 'view';
  S.viewId = S.views[i]?.id || null;
  S.query = '';
  $('q').value = '';
  document.querySelectorAll('.view-item').forEach((el, k) =>
    el.classList.toggle('on', k === i));
  refreshCreate();
  if (!S.viewId) return;
  await loadView();
}

async function loadView() {
  const i = S.views.findIndex((x) => x.id === S.viewId);
  // ?group=1 强制按分组树聚合（对齐手机端矩阵的「按分组」）；折叠状态跨重取保留
  const gp = S.groupMode ? '?group=1' : '';
  S.view = await api('/api/view/' + encodeURIComponent(S.viewId) + gp);
  if (S.view.error) { $('list').innerHTML = `<div class="empty">${esc(S.view.error)}</div>`; return; }
  $('list-title').innerHTML =
    `${esc(S.views[i]?.label || S.viewId)} · ${S.view.rows.length} 行 ` +
    `<button id="view-mode" class="ghost" title="切换表格/卡片模式">${S.viewMode === 'cards' ? '表格' : '卡片'}</button>` +
    `<button id="view-group" class="ghost" title="按分组树聚合显示（可折叠）">${S.groupMode ? '取消分组' : '按分组'}</button>` +
    (hasGroups() ? ` <button id="view-expand" class="ghost" title="展开/折叠全部分组">${S.collapsed.size ? '全部展开' : '全部折叠'}</button>` : '');
  $('view-mode').addEventListener('click', () => {
    S.viewMode = S.viewMode === 'cards' ? 'table' : 'cards';
    try { localStorage.setItem('mneme.viewMode', S.viewMode); } catch (_) {}
    loadView();
  });
  $('view-group').addEventListener('click', () => { S.groupMode = !S.groupMode; loadView(); });
  const ex = $('view-expand');
  if (ex) ex.addEventListener('click', toggleAllGroups);
  renderView();
}

function hasGroups() {
  return (S.view?.rows || []).some((r) => Array.isArray(r._group_path) && r._group_path.length > 1);
}

function toggleAllGroups() {
  if (S.collapsed.size) S.collapsed.clear();
  else collectGroupKeys(groupTreeOf(S.view.rows || []), S.collapsed);
  renderView();
  const ex = $('view-expand');
  if (ex) ex.textContent = S.collapsed.size ? '全部展开' : '全部折叠';
}

function collectGroupKeys(node, into) {
  for (const [name, child] of node.children) {
    into.add(child.key);
    collectGroupKeys(child, into);
  }
}

// 把扁平行按 _group_path 聚成嵌套树；没有路径的行直接挂在 root.rows（未分组）
function groupTreeOf(rows) {
  const root = { children: new Map(), rows: [], key: '' };
  for (const r of rows) {
    const path = Array.isArray(r._group_path) && r._group_path.length ? r._group_path : null;
    if (!path) { root.rows.push(r); continue; }
    let node = root;
    let key = '';
    for (const seg of path) {
      key = key ? key + '>' + seg : seg;
      if (!node.children.has(seg)) node.children.set(seg, { children: new Map(), rows: [], key });
      node = node.children.get(seg);
    }
    node.rows.push(r);
  }
  return root;
}

// ---- 视图表格 / 卡片 ----------------------------------------------------
function renderView() {
  const v = S.view;
  const rows = v.rows || [];
  const tree = groupTreeOf(rows);
  const grouped = tree.children.size > 0;
  // 可见行（折叠生效后）决定 S.items，键盘移动与详情都跟着走
  const visible = [];
  collectVisible(tree, visible);
  const prevSel = S.items[S.sel]?.id;
  // 视图行的主键在不同领域包里可能是 _id / id / 主字段；缺一不可回退，
  // 否则首屏自动开详情会打到 /api/entity/undefined。
  S.items = visible.map((r) => ({
    id: r._id ?? r.id ?? r[v.primary] ?? '',
    title: r[v.primary] || r._id || '',
  }));
  S.sel = Math.max(0, S.items.findIndex((x) => x.id === prevSel));
  if (!S.items.length) S.sel = -1;

  if (S.viewMode === 'cards') {
    renderViewCards(tree, grouped, rows.length);
  } else {
    renderViewTable(tree, grouped, rows.length);
  }
  bindRowClick();
  syncSelection();
  if (S.sel >= 0) showDetail(S.items[S.sel].id);
}

function collectVisible(node, out) {
  for (const child of node.children.values()) {
    if (!S.collapsed.has(child.key)) {
      out.push(...child.rows);
      collectVisible(child, out);
    }
  }
  out.push(...node.rows);
}

function rowCells(r, v) {
  return v.columns.map((c) => {
    const val = r[c.key] || '';
    const cls = c.key === v.primary ? 'name' : 'wrap';
    const tag = c.immutable && val ? '<span class="tag imm">原样</span>' : '';
    return `<td class="${cls}">${tag}${esc(val)}</td>`;
  }).join('');
}

function renderViewTable(tree, grouped, total) {
  const v = S.view;
  const cap = 500;
  const head = `<tr>${v.columns.map((c) => `<th class="${c.key === v.primary ? '' : 'wrap'}">${esc(c.label || c.key)}</th>`).join('')}</tr>`;
  let shown = 0;
  const bodyParts = [];
  // 结构：组头 → 组内直属行 → 再递归子组；root.rows 是未分组行，只在最外层输出一次
  const emitNode = (node, depth, isRoot) => {
    for (const [name, child] of node.children) {
      if (shown >= cap) return;
      const isCollapsed = S.collapsed.has(child.key);
      const n = countRows(child);
      const dot = '·'.repeat(depth);
      bodyParts.push(
        `<tr class="group-row" data-g="${esc(encodeURIComponent(child.key))}"><td colspan="${v.columns.length}">` +
        `<span class="tw">${isCollapsed ? '▸' : '▾'}</span>${dot ? `<span class="muted">${dot} </span>` : ''}${esc(name)} ` +
        `<span class="muted">${n}</span></td></tr>`);
      if (!isCollapsed) {
        for (const r of child.rows) {
          if (shown >= cap) break;
          const idx = S.items.findIndex((x) => x.id === (r._id ?? r.id));
          bodyParts.push(`<tr class="row${idx === S.sel ? ' on' : ''}" data-idx="${idx}">${rowCells(r, v)}</tr>`);
          shown++;
        }
        emitNode(child, depth + 1, false);
      }
    }
    if (isRoot) {
      for (const r of node.rows) {
        if (shown >= cap) break;
        const idx = S.items.findIndex((x) => x.id === (r._id ?? r.id));
        bodyParts.push(`<tr class="row${idx === S.sel ? ' on' : ''}" data-idx="${idx}">${rowCells(r, v)}</tr>`);
        shown++;
      }
    }
  };
  emitNode(tree, 0, true);
  const more = total > cap
    ? `<div class="muted" style="padding:8px 0">还有 ${total - cap} 行没显示，用检索（/）缩小范围</div>` : '';
  $('list').innerHTML =
    `<table>${head}<tbody>${bodyParts.join('') || `<tr><td>没有数据</td></tr>`}</tbody></table>` + more;
  bindGroupToggle();
}

function countRows(node) {
  let n = node.rows.length;
  for (const c of node.children.values()) n += countRows(c);
  return n;
}

function bindGroupToggle() {
  document.querySelectorAll('#list .group-row[data-g]').forEach((el) => {
    el.addEventListener('click', () => {
      const key = decodeURIComponent(el.dataset.g);
      if (S.collapsed.has(key)) S.collapsed.delete(key);
      else S.collapsed.add(key);
      renderView();
    });
  });
}

function renderViewCards(tree, grouped, total) {
  const v = S.view;
  const cap = 300;
  const parts = [];
  let shown = 0;
  const card = (r) => {
    const idx = S.items.findIndex((x) => x.id === (r._id ?? r.id));
    const sub = v.columns.filter((c) => c.key !== v.primary).slice(0, 2)
      .map((c) => r[c.key]).filter(Boolean).map((x) => esc(x)).join('<br>');
    return `<div class="card${idx === S.sel ? ' on' : ''}" data-idx="${idx}">` +
      `<div class="card-name">${esc(r[v.primary] || r._id || '')}</div>` +
      (sub ? `<div class="card-sub">${sub}</div>` : '') +
      `</div>`;
  };
  // 结构：组头 → 组内直属卡片 → 再递归子组；root.rows 是未分组卡片，只在最外层输出一次
  const emitNode = (node, depth, isRoot) => {
    for (const [name, child] of node.children) {
      if (shown >= cap) return;
      const isCollapsed = S.collapsed.has(child.key);
      const dot = '·'.repeat(depth);
      parts.push(
        `<div class="group-row" data-g="${esc(encodeURIComponent(child.key))}">` +
        `<span class="tw">${isCollapsed ? '▸' : '▾'}</span>${dot ? `<span class="muted">${dot} </span>` : ''}${esc(name)} ` +
        `<span class="muted">${countRows(child)}</span></div>`);
      if (!isCollapsed) {
        for (const r of child.rows) {
          if (shown >= cap) break;
          parts.push(card(r)); shown++;
        }
        emitNode(child, depth + 1, false);
      }
    }
    if (isRoot) {
      for (const r of node.rows) {
        if (shown >= cap) break;
        parts.push(card(r)); shown++;
      }
    }
  };
  emitNode(tree, 0, true);
  const more = total > cap
    ? `<div class="muted" style="padding:8px 0">还有 ${total - cap} 条没显示，用检索（/）缩小范围</div>` : '';
  $('list').innerHTML = `<div class="card-grid">${parts.join('') || '<div class="empty">没有数据</div>'}</div>` + more;
  bindGroupToggle();
  document.querySelectorAll('#list .card[data-idx]').forEach((el) => {
    el.addEventListener('click', () => {
      S.sel = +el.dataset.idx;
      syncSelection();
      showDetail(S.items[S.sel].id);
    });
  });
}

// ---- 检索 -------------------------------------------------------------
let searchSeq = 0;
async function doSearch() {
  const q = $('q').value.trim();
  const seq = ++searchSeq;
  if (!q) { leaveSearch(); return; }
  const r = await api('/api/search?query=' + encodeURIComponent(q) + '&limit=50');
  if (seq !== searchSeq) return; // 只认最后一次输入
  if (r.error) { $('list').innerHTML = `<div class="empty">${esc(r.error)}</div>`; return; }
  S.mode = 'search';
  S.hits = r.hits || [];
  S.items = S.hits.map((h) => ({ id: h.id, title: h.name }));
  S.sel = S.items.length ? 0 : -1;
  const modeNote = r.mode === 'or'
    ? ' · 没有一条同时满足所有词，已改为命中任意一个词' : '';
  $('list-title').textContent =
    `检索「${q}」 · ${r.total} 条${modeNote}`;
  $('list').innerHTML = S.hits.length
    ? S.hits.map((h, idx) => `
      <div class="hit${idx === S.sel ? ' on' : ''}" data-idx="${idx}">
        <b>${esc(h.name)}</b> ${h.group ? `<span class="muted">${esc(h.group)}</span>` : ''}
        <div class="snip">${h.via === 'pinyin' ? '<span class="tag">拼音</span>' : h.via === 'synonym' ? '<span class="tag">别名</span>' : ''}${h.label ? esc(h.label) + '：' : ''}${esc(h.snippet || '')}</div>
      </div>`).join('')
    : `<div class="empty">没有匹配。试试更短的词，或拼音首字母（mh → 麻黄）。</div>`;
  bindRowClick();
  syncSelection();
  if (S.sel >= 0) showDetail(S.items[S.sel].id);
}

function leaveSearch() {
  if (S.viewId != null) selectView(S.views.findIndex((v) => v.id === S.viewId));
}

// ---- 选择与详情 -------------------------------------------------------
function bindRowClick() {
  document.querySelectorAll('[data-idx]').forEach((el) => {
    el.addEventListener('click', () => { S.sel = +el.dataset.idx; syncSelection(); showDetail(); });
  });
}

function syncSelection() {
  document.querySelectorAll('[data-idx]').forEach((el) =>
    el.classList.toggle('on', +el.dataset.idx === S.sel));
  const el = document.querySelector(`[data-idx="${S.sel}"]`);
  if (el) el.scrollIntoView({ block: 'nearest' });
}

async function showDetail() {
  const it = S.items[S.sel];
  if (!it || !it.id) {
    $('detail').innerHTML = '<div class="muted">选择一条查看详情</div>';
    return;
  }
  const d = await api('/api/entity/' + encodeURIComponent(it.id));
  if (d.error) { $('detail').innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }
  // 可编辑字段直接渲染成输入框：提交后由服务端 apply_edit 做同值过滤
  // （没变的字段不产生事件），前端不做 diff，避免两端规则分叉。
  $('detail').innerHTML = `
    <h2>${esc(d.title)}</h2>
    <div class="gp">${esc(d.group_path || '未分组')} · rev ${d.rev} · ${esc(d.device)}</div>
    ${(d.fields || []).map((f, i) => f.immutable ? `
      <div class="field">
        <div class="k"><span class="tag imm">原样</span>${esc(f.label || f.key)}</div>
        <div class="v">${esc(f.value) || '<span class="muted">（空）</span>'}</div>
      </div>` : `
      <div class="field">
        <div class="k">${esc(f.label || f.key)}<span class="muted">（${esc(f.kind)}）</span></div>
        <textarea class="fin" data-key="${esc(f.key)}" rows="${f.value.length > 40 || f.kind === 'tags' ? 3 : 1}">${esc(f.value)}</textarea>
      </div>`).join('')}
    <div class="edit-bar">
      <button id="save">保存 <span class="muted">Ctrl+Enter</span></button>
      <button id="undo" class="ghost">撤销上次编辑</button>
      <span class="spacer"></span>
      <button id="del" class="danger">删除</button>
    </div>
    <div class="tool-row" style="margin-top:8px">
      <button class="ghost" data-detail="history">变更历史</button>
      <button class="ghost" data-detail="group">改分组</button>
    </div>
    <div class="muted" id="life-note"></div>`;
  $('save').addEventListener('click', saveEdit);
  $('del').addEventListener('click', () => lifeAction('delete'));
  $('undo').addEventListener('click', () => lifeAction('undo'));
  $('detail').querySelectorAll('.fin').forEach((t) => {
    t.dataset.orig = t.value;
    t.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); saveEdit(); }
    });
  });
}

async function lifeAction(kind) {
  const it = S.items[S.sel];
  if (!it) return;
  const note = $('life-note');
  const label = kind === 'delete' ? '确认删除？删除后进回收站，可恢复' : '';
  if (kind === 'delete' && !confirm(label)) return;
  note.textContent = kind === 'delete' ? '删除中…' : '撤销中…';
  const r = await api('/api/' + kind, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id: it.id }),
  });
  if (r.error) { note.textContent = r.error; return; }
  if (kind === 'delete') {
    note.textContent = '已删除（回收站里可用 /api/restore 恢复）';
    $('detail').innerHTML = '<div class="empty">已删除</div>';
    if (S.mode === 'view' && S.viewId) { await selectView(S.views.findIndex((v) => v.id === S.viewId)); }
  } else {
    note.textContent = `已撤销，恢复 ${(r.restored || []).length} 个字段（rev ${r.rev}）`;
    showDetail();
  }
  const st = await api('/api/state');
  if (!st.error) $('state').textContent = ` · ${st.schema || '未加载领域'} · ${st.entities} 条 · ${st.cards} 张卡 · rev ${st.rev}`;
}

async function saveEdit() {
  const it = S.items[S.sel];
  if (!it) return;
  const fields = {};
  let touched = false;
  $('detail').querySelectorAll('.fin').forEach((t) => {
    // 只提交真的改过的字段；没改的留在请求外，服务端也不再判一次
    if (t.value !== t.dataset.orig) { fields[t.dataset.key] = t.value; touched = true; }
  });
  const note = $('save-note');
  if (!touched) { note.textContent = '没有改动'; return; }
  note.textContent = '保存中…';
  const r = await api('/api/edit', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id: it.id, fields }),
  });
  if (r.error) { note.textContent = ''; note.innerHTML = `<span class="err">${esc(r.error)}</span>`; return; }
  if (r.noop) { note.textContent = '没有实际变化'; return; }
  note.textContent = `已保存 rev ${r.rev}（${(r.changed || []).length} 个字段）；此时「撤销上次编辑」可用`;
  // 刷新详情与列表：列表里的值可能已过期
  const sel = S.sel;
  if (S.mode === 'view' && S.viewId) {
    S.view = await api('/api/view/' + encodeURIComponent(S.viewId));
    renderView();
    S.sel = sel; syncSelection();
  }
  showDetail();
  const btn = $('save');
  if (btn) btn.blur();
}

// ---- 键盘 -------------------------------------------------------------
function move(delta) {
  if (!S.items.length) return;
  S.sel = Math.min(S.items.length - 1, Math.max(0, S.sel + delta));
  syncSelection();
}

document.addEventListener('keydown', (e) => {
  const typing = ['INPUT', 'TEXTAREA'].includes(document.activeElement?.tagName);
  if (T.active) {
    if (e.key === 'Escape') { e.preventDefault(); closeTool(); return; }
    return;
  }
  if (Q.active) {
    if (e.key === 'Escape') { e.preventDefault(); exitQuiz(); return; }
    if (typing) return;
    if (e.key === ' ' || e.key === 'Spacebar') {
      e.preventDefault();
      if (!Q.revealed) revealQuiz();
      return;
    }
    if (['1', '2', '3', '4'].includes(e.key) && Q.revealed) {
      e.preventDefault();
      gradeQuiz(Number(e.key));
      return;
    }
    return; // 复习模式屏蔽矩阵快捷键，避免误改数据
  }
  if (e.key === '/' && !typing) {
    e.preventDefault();
    $('q').focus();
    $('q').select();
    return;
  }
  if (typing) {
    if (e.key === 'Escape') {
      // 在编辑框里按 Esc = 放弃这块输入；在检索框里 = 清空并回到视图
      if (document.activeElement === $('q')) { $('q').value = ''; $('q').blur(); leaveSearch(); }
      else document.activeElement.blur();
    }
    return;
  }
  switch (e.key) {
    case 'j': case 'ArrowDown': e.preventDefault(); move(1); break;
    case 'k': case 'ArrowUp': e.preventDefault(); move(-1); break;
    case 'Enter': e.preventDefault(); showDetail(); break;
    case '[': case ']': {
      e.preventDefault();
      if (!S.views.length) break;
      const cur = S.views.findIndex((v) => v.id === S.viewId);
      const next = e.key === ']' ? (cur + 1) % S.views.length : (cur - 1 + S.views.length) % S.views.length;
      selectView(next);
      break;
    }
    case 'Escape':
      if (S.mode === 'search') { $('q').value = ''; leaveSearch(); }
      break;
  }
});

// 检索防抖：200ms，与手机端一致
let debounce = null;
$('q').addEventListener('input', () => {
  clearTimeout(debounce);
  debounce = setTimeout(doSearch, 200);
});

$('create').addEventListener('click', openCreate);

// ---- 同步 --------------------------------------------------------------
// 状态、预演/真跑、保存配置三件事都在这；引擎在服务端热替换，保存后立刻生效。
async function refreshSync() {
  const st = await api('/api/sync/status');
  const el = $('sync-status');
  if (st.error) { el.textContent = st.error; return; }
  el.textContent = st.configured
    ? `已配置：${(st.targets || []).join('、')}`
    : '未配置（填 owner/repo/token 后保存）';
  const sel = $('sync-target');
  const targets = (st.targets && st.targets.length) ? st.targets : ['github-private'];
  const cur = targets.includes(sel.value) ? sel.value : targets[0];
  sel.innerHTML = targets.map((t) => `<option value="${esc(t)}"${t === cur ? ' selected' : ''}>${esc(t)}</option>`).join('');
  $('sync-dry').disabled = $('sync-run').disabled = !st.configured;
}

async function runSync(dry) {
  const out = $('sync-out');
  out.textContent = dry ? '预演中…' : '同步中…';
  const target = $('sync-target').value || 'github-private';
  const r = await api('/api/sync/' + encodeURIComponent(target) + (dry ? '?dry=true' : ''), { method: 'POST' });
  out.textContent = r.error ? `错误：${r.error}` : (r.report || JSON.stringify(r));
}

async function saveSync() {
  const out = $('sync-out');
  out.textContent = '保存中…';
  const r = await api('/api/sync/config', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      owner: $('sync-owner').value.trim(),
      repo: $('sync-repo').value.trim(),
      path: $('sync-path').value.trim(),
      token: $('sync-token').value.trim(),
    }),
  });
  if (r.error) { out.textContent = `错误：${r.error}`; return; }
  out.textContent = '已保存并启用';
  $('sync-token').value = '';
  refreshSync();
}

$('sync-dry').addEventListener('click', () => runSync(true));
$('sync-run').addEventListener('click', () => runSync(false));
$('sync-save').addEventListener('click', saveSync);
refreshSync();

boot();

// ---- 新建 --------------------------------------------------------------
async function refreshCreate() {
  const t = await api('/api/template');
  if (!t.error) {
    const btn = $('create');
    if (btn) btn.textContent = `＋ 新建${t.kinds.length > 1 ? '（' + t.kind + '）' : ''}`;
  }
}

async function openCreate() {
  if ($('create-mask')) return;
  const t = await api('/api/template');
  if (t.error) { alert(t.error); return; }
  const mask = document.createElement('div');
  mask.id = 'create-mask';
  mask.style.cssText =
    'position:fixed;inset:0;background:rgba(0,0,0,.7);display:flex;' +
    'align-items:center;justify-content:center;z-index:50';
  mask.innerHTML = `
    <form id="create-form" style="background:var(--md-surface-container-low);padding:var(--space-xl);border-radius:var(--shape-l);width:min(92vw,460px);max-height:86vh;overflow:auto;display:flex;flex-direction:column;gap:10px">
      <b>新建${t.kinds.length > 1 ? '（' + esc(t.kind) + '）' : ''}</b>
      ${t.fields.filter((f) => !f.immutable || f.key === t.primary).map((f) => `
        <label style="display:flex;flex-direction:column;gap:4px;font-size:12px">
          ${esc(f.label || f.key)}${f.required ? '<span class="muted">（必填）</span>' : ''}
          <input class="cfin" data-key="${esc(f.key)}" autocomplete="off"
                 style="padding:7px 9px;border-radius:6px;border:1px solid var(--md-outline);background:var(--md-surface-container);color:var(--md-on-surface)" />
        </label>`).join('')}
      <div id="create-err" style="color:var(--md-error);font-size:var(--type-body-medium-size)"></div>
      <div style="display:flex;gap:8px;justify-content:flex-end">
        <button type="button" id="create-cancel" class="ghost">取消</button>
        <button type="submit" id="create-ok">创建</button>
      </div>
    </form>`;
  document.body.appendChild(mask);
  $('create-cancel').addEventListener('click', () => mask.remove());
  $('create-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const fields = {};
    mask.querySelectorAll('.cfin').forEach((inp) => {
      if (inp.value.trim()) fields[inp.dataset.key] = inp.value;
    });
    const r = await api('/api/create', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ kind: t.kind, fields, id_hint: '' }),
    });
    if (r.error) {
      $('create-err').textContent = r.error +
        (r.deleted ? '（在回收站里，可恢复或换个名字）' : '');
      return;
    }
    mask.remove();
    if (S.mode === 'view' && S.viewId) {
      await selectView(S.views.findIndex((v) => v.id === S.viewId));
    }
    // 建完直接打开它的详情，顺手确认一眼
    const idx = S.items.findIndex((x) => x.id === r.id);
    if (idx >= 0) { S.sel = idx; syncSelection(); }
    showDetail();
  });
}

// ---- 密度档：舒适 / 紧凑 -------------------------------------------------
// 只切换 --row-pad / --pane-pad / --font-base 三个变量，组件不必各写一套样式。
(function initDensity() {
  const btn = $('density');
  if (!btn) return;
  const apply = (v) => {
    document.body.dataset.density = v;
    btn.textContent = v === 'compact' ? '舒适' : '紧凑';
    btn.title = v === 'compact' ? '切换到舒适密度' : '切换到紧凑密度';
    try { localStorage.setItem('mneme.density', v); } catch (_) {}
  };
  let saved = 'comfortable';
  try { saved = localStorage.getItem('mneme.density') || 'comfortable'; } catch (_) {}
  apply(saved === 'compact' ? 'compact' : 'comfortable');
  btn.addEventListener('click', () => {
    apply(document.body.dataset.density === 'compact' ? 'comfortable' : 'compact');
  });
})();

// ---- 明暗主题：与手机端共用同一套 M3 语义色，偏好记在本机 -------------
// 三态循环：亮 → 暗 → 跟随系统（system 态实时跟随 prefers-color-scheme，
// 与手机端 MnemeAppearance.mode 同口径）。?theme= 深链仍可强制指定。
(function initTheme() {
  const btn = $('theme');
  if (!btn) return;
  const NEXT = { light: 'dark', dark: 'system', system: 'light' };
  const LABEL = { light: '亮色', dark: '暗色', system: '跟随' };
  const mq = window.matchMedia ? window.matchMedia('(prefers-color-scheme: dark)') : null;
  let mode = 'dark'; // 当前偏好态（不是渲染后的明暗）
  const apply = () => {
    const v = mode === 'system' ? (mq && mq.matches ? 'dark' : 'light') : mode;
    document.body.dataset.theme = v;
    btn.textContent = LABEL[mode];
    btn.title = `当前：${LABEL[mode]}（点按切到${LABEL[NEXT[mode]]}）`;
  };
  const forced = new URLSearchParams(location.search).get('theme');
  if (forced && ['light', 'dark', 'system'].includes(forced)) {
    mode = forced; // 深链只影响本次渲染，不改本机偏好
  } else {
    try { mode = localStorage.getItem('mneme.theme') || 'dark'; } catch (_) {}
    // 老存档只有 light/dark，system 是新增第三态
    if (!['light', 'dark', 'system'].includes(mode)) mode = 'dark';
  }
  apply();
  if (mq) mq.addEventListener('change', () => { if (mode === 'system') apply(); });
  btn.addEventListener('click', () => {
    mode = NEXT[mode] || 'light';
    try { localStorage.setItem('mneme.theme', mode); } catch (_) {}
    apply();
  });
})();

// ---- 复习模式：题面 / 答案 / 四档评分全部走 Rust 侧 /api/quiz* -----------
// 业务规则（到期判定、FSRS、判分口径）一律不在这里重写，与手机端同源。
// plan = 按复习参数出题；weak = 错题本（末尾连续答错 ≥2 次，连对两次移出）。
const Q = { active: false, mode: 'plan', cards: [], idx: 0, due: 0, revealed: false, busy: false };
const GRADE_LABEL = { 1: '忘了', 2: '困难', 3: '良好', 4: '简单' };

function showQuizPane(on) {
  Q.active = on;
  $('quiz-pane').hidden = !on;
  document.querySelector('main').style.display = on ? 'none' : '';
  if (!on) $('quiz-cfg').hidden = true;
}

async function startQuiz(mode = 'plan') {
  showQuizPane(true);
  Q.mode = mode;
  Q.cards = []; Q.idx = 0; Q.revealed = false;
  if (mode === 'weak') await loadWeakCards();
  else await loadQuizCards();
}

function exitQuiz() {
  showQuizPane(false);
  Q.busy = false;
}

function renderQuizStats(r) {
  $('quiz-stats').textContent =
    `到期 ${r.due} · 新卡 ${r.new} · 薄弱 ${r.weak} · 水蛭 ${r.leech}` +
    (r.retention != null ? ` · 留存 ${Math.round(r.retention * 100)}%` : '');
}

async function loadQuizCards() {
  $('quiz-note').textContent = '取题中…';
  const r = await api('/api/quiz/plan');
  if (r.error) {
    $('quiz-note').textContent = '错误：' + r.error;
    Q.cards = [];
    renderQuiz();
    return;
  }
  renderQuizStats(r);
  Q.cards = r.cards || [];
  Q.due = r.due || 0;
  Q.idx = 0;
  Q.revealed = false;
  renderQuiz();
  $('quiz-note').textContent = Q.cards.length ? '' : '今天没有待复习卡片。';
}

async function loadWeakCards() {
  $('quiz-note').textContent = '取错题本…';
  const r = await api('/api/weak-cards?limit=50');
  if (r.error) {
    $('quiz-note').textContent = '错误：' + r.error;
    Q.cards = [];
    renderQuiz();
    return;
  }
  $('quiz-stats').textContent = '';
  Q.cards = (r.items || []).map((x) => ({ ...x, weak: true }));
  Q.due = 0;
  Q.idx = 0;
  Q.revealed = false;
  renderQuiz();
  $('quiz-note').textContent = Q.cards.length ? '' : '错题本是空的，连对两次自然移出。';
}

function renderQuiz() {
  const c = Q.cards[Q.idx];
  const label = Q.mode === 'weak' ? '错题本' : '今日队列';
  $('quiz-progress').textContent = Q.cards.length
    ? `${label} ${Q.cards.length} 张 · 第 ${Q.idx + 1} 张`
    : `${label} 0 张`;
  $('quiz-dir').textContent = c
    ? (c.direction ? '方向：' + c.direction : '反向卡') +
      (c.misses ? ` · 连错 ${c.misses} 次` : '')
    : '';
  // 薄弱模式下题面就是实体名本身（错题本先看名字回忆功效），不额外藏
  $('quiz-prompt').textContent = c ? c.prompt : (Q.mode === 'weak' ? '错题本是空的' : '没有待复习卡片');
  $('quiz-answer').hidden = true;
  $('quiz-answer').innerHTML = '';
  renderQuizSrs(c);
  $('quiz-reveal').hidden = !c || Q.revealed;
  $('quiz-reveal').disabled = !c;
  $('quiz-judge').hidden = true;
  $('quiz-input').value = '';
  $('quiz-judge-out').textContent = '';
  $('quiz-grades').hidden = !c || !Q.revealed;
}

function renderQuizSrs(c) {
  const el = $('quiz-srs');
  if (!c || c.stability == null) { el.hidden = true; el.innerHTML = ''; return; }
  const chips = [
    `稳定 ${c.stability} 天`,
    `难度 ${c.difficulty}`,
    `可提取 ${c.retrievability}%`,
    `遗忘 ${c.lapses} 次`,
  ];
  if (c.leech) chips.push('水蛭卡');
  el.innerHTML = chips.map((x) => `<span class="srs-chip${c.leech && x === '水蛭卡' ? ' srs-leech' : ''}">${esc(x)}</span>`).join('');
  el.hidden = false;
}

function asText(v) {
  if (Array.isArray(v)) return v.filter(Boolean).join(' / ');
  if (v == null) return '';
  const t = String(v).trim();
  return t === '[]' ? '' : t;
}

async function revealQuiz() {
  const c = Q.cards[Q.idx];
  if (!c || Q.revealed || Q.busy) return;
  Q.busy = true;
  $('quiz-note').textContent = '读取答案…';
  const r = await api('/api/quiz/answer/' + encodeURIComponent(c.id));
  Q.busy = false;
  if (r.error) { $('quiz-note').textContent = '错误：' + r.error; return; }
  const lines = [`<div>${esc(r.answer || '')}</div>`];
  const accept = asText(r.accept);
  const boundary = asText(r.boundary);
  if (accept) lines.push(`<div class="accept">可接受：${esc(accept)}</div>`);
  if (boundary) lines.push(`<div class="accept">边界：${esc(boundary)}</div>`);
  $('quiz-answer').innerHTML = lines.join('');
  $('quiz-answer').hidden = false;
  Q.revealed = true;
  $('quiz-reveal').hidden = true;
  $('quiz-grades').hidden = false;
  $('quiz-judge').hidden = false;
  $('quiz-note').textContent = '可先自己写答案判分，或直接按 1-4 评分';
}

async function gradeQuiz(g) {
  const c = Q.cards[Q.idx];
  if (!c || !Q.revealed || Q.busy) return;
  Q.busy = true;
  $('quiz-note').textContent = '记录中…';
  const r = await api('/api/review', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ card_id: c.id, grade: g }),
  });
  Q.busy = false;
  if (r.error) { $('quiz-note').textContent = '错误：' + r.error; return; }
  Q.revealed = false;
  Q.idx++;
  let note = `已记录：${GRADE_LABEL[g]}`;
  const m = r.mastery;
  if (m && m.changed) note += ` · 掌握度：${m.prev || '（空）'} → ${m.label}`;
  else if (m && m.label) note += ` · 掌握度：${m.label}`;
  if (Q.idx >= Q.cards.length) {
    if (Q.mode === 'weak') await loadWeakCards();
    else await loadQuizCards();
    $('quiz-note').textContent = note + ' · 本轮完成';
  } else {
    $('quiz-note').textContent = note;
    renderQuiz();
  }
}

// ---- 复习参数：读写 review.json，与手机端同一份配置 ----------------------
async function openQuizCfg() {
  const box = $('quiz-cfg');
  if (!box.hidden) { box.hidden = true; return; }
  $('quiz-cfg-out').textContent = '';
  const r = await api('/api/review-config');
  if (r.error) { $('quiz-cfg-out').textContent = '错误：' + r.error; box.hidden = false; return; }
  $('cfg-new').value = r.new_per_day ?? 10;
  $('cfg-due').value = r.reviews_per_day ?? 100;
  $('cfg-session').value = r.session_limit ?? 20;
  $('cfg-retention').value = r.desired_retention ?? 0.9;
  $('cfg-leech').value = r.leech_threshold ?? 8;
  box.hidden = false;
}

async function saveQuizCfg() {
  const body = {
    new_per_day: Number($('cfg-new').value) || 0,
    reviews_per_day: Number($('cfg-due').value) || 0,
    session_limit: Number($('cfg-session').value) || 0,
    desired_retention: Number($('cfg-retention').value) || 0.9,
    leech_threshold: Number($('cfg-leech').value) || 8,
  };
  const r = await api('/api/review-config', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  $('quiz-cfg-out').textContent = r.error
    ? '错误：' + r.error
    : '已保存，下次取题生效';
  if (!r.error) $('quiz-cfg').hidden = true;
}

$('quiz-toggle').addEventListener('click', () => startQuiz('plan'));
$('quiz-exit').addEventListener('click', exitQuiz);
$('quiz-reveal').addEventListener('click', revealQuiz);
$('quiz-weak').addEventListener('click', () => startQuiz('weak'));
$('quiz-cfg-btn').addEventListener('click', openQuizCfg);
$('quiz-cfg-save').addEventListener('click', saveQuizCfg);
document.querySelectorAll('#quiz-grades button[data-grade]').forEach((b) => {
  b.addEventListener('click', () => gradeQuiz(Number(b.dataset.grade)));
});
// 深链：?quiz=1 直接进复习（&weak=1 走错题本），便于分享与无头截图验证
{
  const qp = new URLSearchParams(location.search);
  if (qp.get('quiz') === '1') {
    const mode = qp.get('weak') === '1' ? 'weak' : 'plan';
    startQuiz(mode).then(() => {
      if (qp.get('reveal') === '1') revealQuiz();
      if (qp.get('cfg') === '1') openQuizCfg();
    });
  }
}

// ---- 工具覆盖层：把手机端能力接进桌面（全部走 /api/*） -----------------
// 与复习面板并列：同一时间只显示一个覆盖层，Esc 关闭。
const T = { active: false, name: '', data: null, arg: '', decisions: {} };
const TOOL_TITLES = {
  reverse: '反查线索', data: '导入与导出', deleted: '回收站',
  groups: '分组管理', paste: '批量粘贴', upstream: '公共基础库', history: '变更历史',
};

function toolBody(html) {
  $('tool-body').innerHTML = html + '<pre id="tool-out" class="tool-note"></pre>';
}
function toolNote(msg, isErr) {
  const el = $('tool-out');
  if (!el) return;
  el.textContent = msg ?? '';
  el.className = isErr ? 'tool-note tool-error' : 'tool-note';
}
function showToolPane(on) {
  T.active = on;
  $('tool-pane').hidden = !on;
  document.querySelector('main').style.display = on ? 'none' : '';
}
async function openTool(name, arg = '') {
  // 复习与工具互斥，避免两个覆盖层叠在一起
  if (Q.active) exitQuiz();
  T.name = name; T.arg = arg; T.data = null; T.decisions = {};
  $('tool-title').textContent = TOOL_TITLES[name] || '工具';
  showToolPane(true);
  toolBody('<div class="muted">读取中…</div>');
  try {
    if (name === 'reverse') await renderReverse();
    else if (name === 'data') await renderData();
    else if (name === 'deleted') await renderDeleted();
    else if (name === 'groups') await renderGroups(arg);
    else if (name === 'paste') await renderPaste();
    else if (name === 'upstream') await renderUpstream();
    else if (name === 'history') await renderHistory(arg);
  } catch (e) { toolNote('加载失败：' + e, true); }
}
function closeTool() { showToolPane(false); }

function renderReverse() {
  toolBody(`<div class="tool-card">
    <h3>按线索反推实体</h3>
    <div class="tool-row"><input type="text" id="t-rev" placeholder="功效:宣肺平喘|禁忌:表虚自汗" style="flex:1 1 320px" /><button data-act="reverse-run">反查</button></div>
    <div id="t-list" class="tool-list"></div>
  </div>`);
  toolNote('多个线索用 | 分隔，全部命中才返回');
}
async function reverseRun() {
  const q = $('t-rev').value.trim();
  if (!q) return toolNote('先填线索', true);
  const r = await api('/api/reverse?clue=' + encodeURIComponent(q));
  if (r.error) return toolNote('错误：' + r.error, true);
  const items = r.items || [];
  $('t-list').innerHTML = items.length ? items.map((x) => `<div class="tool-item">
    <span class="tag">${esc(String(x.score ?? ''))}</span>
    <span class="grow"><b>${esc(x.name || x.id || '')}</b><div class="snip">${esc(x.summary || '')}</div></span>
  </div>`).join('') : '<div class="muted">没有匹配</div>';
  toolNote(`${items.length} 条结果`);
}

function renderData() {
  const fmts = ['bundle', 'json', 'csv', 'tsv', 'md', 'anki', 'html'];
  toolBody(`<div class="tool-card"><h3>导出</h3>
    <div class="tool-row"><select id="t-fmt">${fmts.map((f) => `<option value="${f}">${f}</option>`).join('')}</select>
    <button data-act="export">下载</button></div></div>
    <div class="tool-card"><h3>导入</h3>
    <div class="tool-row"><input type="file" id="t-file" /><button data-act="import-preview" class="ghost">预览差异</button><button data-act="import-run">确认导入</button></div>
    <div class="muted">导入走 Rust 侧三方合并；原样字段冲突会提示，不会静默覆盖。</div></div>`);
  toolNote('先选文件，预览后再导入');
}
async function toolFile() {
  const f = $('t-file')?.files?.[0];
  if (!f) return null;
  return { content: await f.text(), filename: f.name };
}
async function toolImport(dry) {
  const f = await toolFile();
  if (!f) return toolNote('先选文件', true);
  toolNote(dry ? '预览中…' : '导入中…');
  const r = await api(dry ? '/api/import/preview' : '/api/import', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ...f, dry }),
  });
  toolNote(r.error ? '错误：' + r.error : JSON.stringify(r, null, 2), !!r.error);
}
async function toolExport() {
  const fmt = $('t-fmt').value;
  const r = await fetch('/api/export?format=' + encodeURIComponent(fmt) + '&token=' + encodeURIComponent(TOKEN));
  if (!r.ok) return toolNote('导出失败：HTTP ' + r.status, true);
  const blob = await r.blob();
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = 'mneme-export.' + fmt;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  toolNote('已下载 ' + fmt);
}

async function renderDeleted() {
  toolBody('<div class="tool-card"><h3>回收站（墓碑，可恢复）</h3><div id="t-list" class="tool-list"></div></div>');
  await refreshDeleted();
}
async function refreshDeleted() {
  const r = await api('/api/deleted');
  if (r.error) return toolNote('错误：' + r.error, true);
  const items = r.items || [];
  $('t-list').innerHTML = items.length ? items.map((x) => `<div class="tool-item">
    <span class="grow"><b>${esc(x.name)}</b><div class="snip">${esc(x.id)} · rev ${x.rev}</div></span>
    <button class="ghost" data-act="restore" data-id="${esc(x.id)}">恢复</button>
  </div>`).join('') : '<div class="muted">回收站是空的</div>';
  toolNote(`${items.length} 条`);
}

async function renderGroups(arg) {
  const r = await api('/api/groups');
  if (r.error) { toolBody(`<div class="tool-card tool-error">${esc(r.error)}</div>`); return; }
  T.data = r;
  const groups = r.items || [];
  const opts = groups.map((g) => `<option value="${esc(g.id)}">${esc(g.path || g.name)}</option>`).join('');
  const rows = groups.length ? groups.map((g) => `<div class="tool-item">
    <span class="grow" style="padding-left:${(g.depth || 0) * 16}px"><b>${esc(g.name)}</b>
    <div class="snip">${esc(g.path || g.id)} · ${g.entities} 条</div></span>
    <button class="ghost" data-act="group-edit" data-id="${esc(g.id)}" data-name="${esc(g.name)}" data-parent="${esc(g.parent || '')}">编辑</button>
    <button class="danger" data-act="group-del" data-id="${esc(g.id)}">删除</button>
  </div>`).join('') : '<div class="muted">还没有分组</div>';
  toolBody(`<div class="tool-card"><h3>分组树（${r.total} 组 · 未分组 ${r.unassigned}）</h3>
    <div id="t-list" class="tool-list">${rows}</div>
    <h3>新建 / 重命名</h3>
    <div class="tool-row"><input type="text" id="t-g-id" placeholder="分组 id（留空=用名称）" />
    <input type="text" id="t-g-name" placeholder="名称" />
    <select id="t-g-parent"><option value="">（无上级）</option>${opts}</select>
    <button data-act="group-upsert">保存分组</button></div>
    ${arg ? `<h3>把「${esc(arg)}」挂到分组</h3>
    <div class="tool-row"><select id="t-g-assign"><option value="">（移出分组）</option>${opts}</select>
    <button data-act="group-assign" data-id="${esc(arg)}">应用</button></div>` : ''}
  </div>`);
  toolNote(groups.length ? '删除分组会把子组上移、成员变未分组' : '');
}

async function renderPaste() {
  const t = await api('/api/template');
  T.data = { blocks: [], kind: t.kind || 'herb' };
  toolBody(`<div class="tool-card"><h3>批量粘贴（空行分隔多条）</h3>
    <textarea id="t-paste" placeholder="药名：麻黄&#10;功效：发汗解表&#10;&#10;药名：桂枝&#10;功效：温通经脉"></textarea>
    <div class="tool-row"><button data-act="paste-parse">解析</button><button data-act="paste-dup" class="ghost">查重</button><button data-act="paste-create">批量新建</button></div>
    <div id="t-list" class="tool-list"></div></div>`);
  toolNote('解析后确认字段，再批量新建');
}
async function pasteParse() {
  const r = await api('/api/parse-paste', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ text: $('t-paste').value, kind: T.data.kind }),
  });
  if (r.error) return toolNote('错误：' + r.error, true);
  T.data.blocks = r.blocks || [];
  $('t-list').innerHTML = T.data.blocks.length ? T.data.blocks.map((b) => `<div class="tool-item">
    <span class="grow"><b>第 ${b.index + 1} 条 · 命中 ${b.matched} 个字段</b>
    <div class="snip">${esc(JSON.stringify(b.fields))}${b.unknown?.length ? ' · 未识别：' + esc(b.unknown.join(',')) : ''}</div></span>
  </div>`).join('') : '<div class="muted">没有解析出条目（用空行分隔多条）</div>';
  toolNote(`共 ${T.data.blocks.length} 条，跳过 ${r.skipped_blocks ?? 0} 块`);
}
function pasteFields(b) {
  const out = {};
  for (const [k, v] of Object.entries(b?.fields || {})) out[k] = typeof v === 'string' ? v : JSON.stringify(v);
  return out;
}
async function pasteDup() {
  const b = T.data.blocks?.[0];
  if (!b) return toolNote('先解析', true);
  const r = await api('/api/check-duplicate', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ kind: T.data.kind, fields: pasteFields(b), exclude_id: '' }),
  });
  toolNote(r.error ? '错误：' + r.error : `疑似重复 ${(r.items || []).length} 条：\n` +
    (r.items || []).map((x) => `${x.name}（${x.reason}）`).join('\n'));
}
async function pasteCreate() {
  if (!T.data.blocks?.length) return toolNote('先解析', true);
  const r = await api('/api/create-batch', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ kind: T.data.kind, items: T.data.blocks.map((b) => ({ fields: b.fields })) }),
  });
  toolNote(r.error ? '错误：' + r.error :
    `新建 ${(r.created || []).length} 条 / 跳过 ${(r.skipped || []).length} 条 / 失败 ${(r.failed || []).length} 条`);
}

async function renderHistory(id) {
  toolBody(`<div class="tool-card"><h3>${esc(id || '')} 的变更历史</h3><div id="t-list" class="tool-list"></div></div>`);
  if (!id) return toolNote('没有指定实体', true);
  const r = await api('/api/history/' + encodeURIComponent(id));
  if (r.error) return toolNote('错误：' + r.error, true);
  const items = r.items || [];
  $('t-list').innerHTML = items.length ? items.map((x) => `<div class="tool-item"><span class="grow snip">${esc(typeof x === 'string' ? x : JSON.stringify(x))}</span></div>`).join('') : '<div class="muted">暂无历史</div>';
  toolNote(`${items.length} 条`);
}

async function renderUpstream() {
  toolBody(`<div class="tool-card"><h3>公共基础库（只读上游）</h3>
    <div id="t-status" class="muted">读取中…</div>
    <div class="tool-row"><button data-act="up-pull">拉取公共库</button>
    <button data-act="up-suggest" class="ghost">全部按建议</button>
    <button data-act="up-resolve">提交裁决</button></div>
    <div id="t-list" class="tool-list"></div></div>`);
  const st = await api('/api/upstream/status');
  $('t-status').textContent = st.error ? st.error :
    `上游 rev ${st.upstream_rev} · 跟踪 ${st.tracked_entities} 条 · 待裁决计划 ${st.pending_plans} · 源 ${st.public_source || '未配置'}`;
  toolNote('拉取会生成暂存计划；有冲突需逐项决定后才会写库');
}
async function upPull() {
  toolNote('拉取中…');
  const r = await api('/api/upstream/pull', { method: 'POST' });
  if (r.error) return toolNote('错误：' + r.error, true);
  if (r.unchanged) return toolNote('上游未变化（rev ' + r.upstream_rev + '）');
  T.data = r;
  T.decisions = {};
  renderConflicts(r);
}
function renderConflicts(plan) {
  const cs = plan.conflicts || [];
  $('t-list').innerHTML = cs.length ? cs.map((c) => `<div class="tool-item" style="align-items:flex-start;flex-direction:column">
    <b>${esc(c.entity_name)} · ${esc(c.label || c.field)}</b>
    <div class="conflict-grid"><div>基线：${esc(c.base || '（空）')}</div><div>上游：${esc(c.upstream || '（空）')}</div><div>我的：${esc(c.local || '（空）')}</div></div>
    <div class="conflict-choices">
      <button class="ghost" data-act="decide" data-cid="${esc(c.id)}" data-v="KeepMine">保留我的</button>
      <button class="ghost" data-act="decide" data-cid="${esc(c.id)}" data-v="TakeUpstream">用上游</button>
      <button class="ghost" data-act="decide" data-cid="${esc(c.id)}" data-v="KeepBoth">两者都留</button>
      <input type="text" data-custom="${esc(c.id)}" placeholder="自定义值（可选）" />
    </div>
    <div class="snip" data-state="${esc(c.id)}">未决定 · 建议 ${esc(String(c.suggestion ?? ''))}</div>
  </div>`).join('') : '<div class="muted">没有需要裁决的冲突，可直接提交</div>';
  toolNote(plan.summary || `计划 ${plan.id}：冲突 ${cs.length} 项`);
}
function markDecision(cid, v) {
  const custom = $('tool-body').querySelector(`[data-custom="${CSS.escape(cid)}"]`)?.value.trim() || '';
  T.decisions[cid] = { decision: v, custom_value: custom || null };
  const st = $('tool-body').querySelector(`[data-state="${CSS.escape(cid)}"]`);
  if (st) st.textContent = `已决定：${v}${custom ? ' · 自定义 ' + custom : ''}`;
  toolNote(`已决定 ${Object.keys(T.decisions).length} 项`);
}
async function upResolve() {
  if (!T.data?.id) return toolNote('先拉取公共库', true);
  const decisions = Object.entries(T.decisions).map(([conflict_id, v]) => ({ conflict_id, ...v }));
  const r = await api('/api/upstream/resolve', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ plan_id: T.data.id, decisions }),
  });
  toolNote(r.error ? '错误：' + r.error : JSON.stringify(r, null, 2), !!r.error);
}

$('tool-exit').addEventListener('click', closeTool);
document.addEventListener('click', (e) => {
  const t = e.target.closest?.('[data-tool]');
  if (t) { openTool(t.dataset.tool); return; }
  const d = e.target.closest?.('[data-detail]');
  if (d) {
    const id = S.items[S.sel]?.id;
    if (!id) return;
    openTool(d.dataset.detail === 'history' ? 'history' : 'groups', id);
  }
});
$('tool-body').addEventListener('click', async (e) => {
  const btn = e.target.closest?.('[data-act]');
  if (!btn) return;
  const act = btn.dataset.act;
  try {
    if (act === 'reverse-run') await reverseRun();
    else if (act === 'export') await toolExport();
    else if (act === 'import-preview') await toolImport(true);
    else if (act === 'import-run') await toolImport(false);
    else if (act === 'restore') {
      const r = await api('/api/restore', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: btn.dataset.id }) });
      toolNote(r.error ? '错误：' + r.error : '已恢复 ' + btn.dataset.id, !!r.error);
      await refreshDeleted();
    } else if (act === 'group-edit') {
      $('t-g-id').value = btn.dataset.id; $('t-g-name').value = btn.dataset.name;
      $('t-g-parent').value = btn.dataset.parent || '';
      toolNote('已填入编辑框，改完点「保存分组」');
    } else if (act === 'group-upsert') {
      const r = await api('/api/groups/upsert', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
        id: $('t-g-id').value.trim(), name: $('t-g-name').value.trim(), parent: $('t-g-parent').value,
      }) });
      toolNote(r.error ? '错误：' + r.error : '已保存分组 ' + r.id, !!r.error);
      if (!r.error) await renderGroups(T.arg);
    } else if (act === 'group-del') {
      if (!confirm('删除该分组？子组会上移，成员变未分组')) return;
      const r = await api('/api/groups/delete', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: btn.dataset.id }) });
      toolNote(r.error ? '错误：' + r.error : '已删除 ' + btn.dataset.id, !!r.error);
      if (!r.error) await renderGroups(T.arg);
    } else if (act === 'group-assign') {
      const r = await api('/api/group', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: btn.dataset.id, group_id: $('t-g-assign').value }) });
      toolNote(r.error ? '错误：' + r.error : '已更新分组', !!r.error);
    } else if (act === 'paste-parse') await pasteParse();
    else if (act === 'paste-dup') await pasteDup();
    else if (act === 'paste-create') await pasteCreate();
    else if (act === 'up-pull') await upPull();
    else if (act === 'up-suggest') {
      (T.data?.conflicts || []).forEach((c) => markDecision(c.id, String(c.suggestion || 'KeepMine')));
    } else if (act === 'up-resolve') await upResolve();
    else if (act === 'decide') markDecision(btn.dataset.cid, btn.dataset.v);
  } catch (err) { toolNote('操作失败：' + err, true); }
});

// 复习：自由输入判分（与网页端 /api/judge 同源）
async function judgeQuiz() {
  const c = Q.cards[Q.idx];
  const input = $('quiz-input').value.trim();
  if (!c || !input) return;
  const r = await api('/api/judge', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ card_id: c.id, input }),
  });
  $('quiz-judge-out').textContent = r.error ? '错误：' + r.error :
    (r.summary || `相似度 ${r.score ?? '?'} · ${r.correct ? '命中' : '未命中'}`);
}
$('quiz-judge-btn').addEventListener('click', judgeQuiz);
$('quiz-input').addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); judgeQuiz(); } });

// 深链：?tool=reverse|data|deleted|groups|paste|upstream 便于验证与分享
{
  const tp = new URLSearchParams(location.search).get('tool');
  if (tp && TOOL_TITLES[tp]) openTool(tp);
}
