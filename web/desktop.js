// mneme 桌面前端：三栏 + 键盘优先。
//
// 设计约束（与现有网页端不同）：桌面是「键盘 + 多列 + 双栏详情」，不是单列触控。
// 但数据层完全复用 /api/*，检索与详情的口径都在 Rust 侧（search / detail_view），
// 这里只做渲染与键位绑定 —— 两端不写两份业务规则。
//
// 键位：/ 检索 · j/k 或 ↑↓ 移动 · Enter 详情 · [ ] 切视图 · Esc 收起

const TOKEN = new URLSearchParams(location.search).get('token') || '';
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));

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
};

// ---- 启动 -------------------------------------------------------------
async function boot() {
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
  if (S.views.length) selectView(0);
}

async function selectView(i) {
  S.mode = 'view';
  S.viewId = S.views[i]?.id || null;
  S.query = '';
  $('q').value = '';
  document.querySelectorAll('.view-item').forEach((el, k) =>
    el.classList.toggle('on', k === i));
  if (!S.viewId) return;
  S.view = await api('/api/view/' + encodeURIComponent(S.viewId));
  if (S.view.error) { $('list').innerHTML = `<div class="empty">${esc(S.view.error)}</div>`; return; }
  $('list-title').textContent = `${S.views[i].label || S.viewId} · ${S.view.rows.length} 行`;
  renderView();
}

// ---- 视图表格 ----------------------------------------------------------
function renderView() {
  const v = S.view;
  const rows = v.rows || [];
  S.items = rows.map((r) => ({ id: r._id, title: r[v.primary] || r._id }));
  S.sel = S.items.length ? 0 : -1;
  const cap = 500;
  const shown = rows.slice(0, cap);
  const head = `<tr>${v.columns.map((c) => `<th class="${c.key === v.primary ? '' : 'wrap'}">${esc(c.label || c.key)}</th>`).join('')}</tr>`;
  let lastGroup = null;
  const body = shown.map((r, idx) => {
    const cells = v.columns.map((c) => {
      const val = r[c.key] || '';
      const cls = c.key === v.primary ? 'name' : 'wrap';
      const tag = c.immutable && val ? '<span class="tag imm">原样</span>' : '';
      return `<td class="${cls}">${tag}${esc(val)}</td>`;
    }).join('');
    let groupRow = '';
    if (v.group_by && r._group !== lastGroup) {
      lastGroup = r._group;
      groupRow = `<tr class="group-row"><td colspan="${v.columns.length}">${esc(r._group || '未分组')}</td></tr>`;
    }
    return groupRow +
      `<tr class="row${idx === S.sel ? ' on' : ''}" data-idx="${idx}">${cells}</tr>`;
  }).join('');
  const more = rows.length > cap
    ? `<div class="muted" style="padding:8px 0">还有 ${rows.length - cap} 行没显示，用检索（/）缩小范围</div>` : '';
  $('list').innerHTML =
    `<table>${head}<tbody>${body || `<tr><td>没有数据</td></tr>`}</tbody></table>` + more;
  bindRowClick();
  syncSelection();
  if (S.sel >= 0) showDetail(S.items[S.sel].id);
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
        <div class="snip">${h.via === 'pinyin' ? '<span class="tag">拼音</span>' : ''}${h.label ? esc(h.label) + '：' : ''}${esc(h.snippet || '')}</div>
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
  if (!it) return;
  const d = await api('/api/entity/' + encodeURIComponent(it.id));
  if (d.error) { $('detail').innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }
  $('detail').innerHTML = `
    <h2>${esc(d.title)}</h2>
    <div class="gp">${esc(d.group_path || '未分组')} · rev ${d.rev} · ${esc(d.device)}</div>
    ${(d.fields || []).map((f) => `
      <div class="field">
        <div class="k">${f.immutable ? '<span class="tag imm">原样</span>' : ''}${esc(f.label || f.key)}</div>
        <div class="v">${esc(f.value) || '<span class="muted">（空）</span>'}</div>
      </div>`).join('')}`;
}

// ---- 键盘 -------------------------------------------------------------
function move(delta) {
  if (!S.items.length) return;
  S.sel = Math.min(S.items.length - 1, Math.max(0, S.sel + delta));
  syncSelection();
}

document.addEventListener('keydown', (e) => {
  const typing = ['INPUT', 'TEXTAREA'].includes(document.activeElement?.tagName);
  if (e.key === '/' && !typing) {
    e.preventDefault();
    $('q').focus();
    $('q').select();
    return;
  }
  if (typing) {
    if (e.key === 'Escape') { $('q').value = ''; $('q').blur(); leaveSearch(); }
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

boot();
