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
  refreshCreate();
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
  // 视图行的主键在不同领域包里可能是 _id / id / 主字段；缺一不可回退，
  // 否则首屏自动开详情会打到 /api/entity/undefined。
  S.items = rows.map((r) => ({
    id: r._id ?? r.id ?? r[v.primary] ?? '',
    title: r[v.primary] || r._id || '',
  }));
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
  $('sync-dry').disabled = $('sync-run').disabled = !st.configured;
}

async function runSync(dry) {
  const out = $('sync-out');
  out.textContent = dry ? '预演中…' : '同步中…';
  const r = await api('/api/sync/github-private' + (dry ? '?dry=true' : ''), { method: 'POST' });
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
(function initTheme() {
  const btn = $('theme');
  if (!btn) return;
  const apply = (v) => {
    document.body.dataset.theme = v;
    btn.textContent = v === 'light' ? '暗色' : '亮色';
    btn.title = v === 'light' ? '切换到暗色主题' : '切换到亮色主题';
    try { localStorage.setItem('mneme.theme', v); } catch (_) {}
  };
  const forced = new URLSearchParams(location.search).get('theme');
  let saved = forced || 'dark';
  if (!forced) {
    try { saved = localStorage.getItem('mneme.theme') || 'dark'; } catch (_) {}
  }
  apply(saved === 'light' ? 'light' : 'dark');
  btn.addEventListener('click', () => {
    apply(document.body.dataset.theme === 'light' ? 'dark' : 'light');
  });
})();

// ---- 复习模式：题面 / 答案 / 四档评分全部走 Rust 侧 /api/quiz* -----------
// 业务规则（到期判定、SM2、判分口径）一律不在这里重写，与手机端同源。
const Q = { active: false, cards: [], idx: 0, due: 0, revealed: false, busy: false };
const GRADE_LABEL = { 1: '忘了', 2: '困难', 3: '良好', 4: '简单' };

function showQuizPane(on) {
  Q.active = on;
  $('quiz-pane').hidden = !on;
  document.querySelector('main').style.display = on ? 'none' : '';
}

async function startQuiz() {
  showQuizPane(true);
  Q.cards = []; Q.idx = 0; Q.revealed = false;
  await loadQuizCards();
}

function exitQuiz() {
  showQuizPane(false);
  Q.busy = false;
}

async function loadQuizCards() {
  $('quiz-note').textContent = '取题中…';
  const r = await api('/api/quiz?limit=10');
  if (r.error) {
    $('quiz-note').textContent = '错误：' + r.error;
    Q.cards = [];
    renderQuiz();
    return;
  }
  Q.cards = r.cards || [];
  Q.due = r.due || 0;
  Q.idx = 0;
  Q.revealed = false;
  renderQuiz();
  $('quiz-note').textContent = Q.cards.length ? '' : '今天没有待复习卡片。';
}

function renderQuiz() {
  const c = Q.cards[Q.idx];
  $('quiz-progress').textContent = Q.cards.length
    ? `今日 ${Q.due} 张 · 第 ${Q.idx + 1}/${Q.cards.length} 张`
    : `今日 ${Q.due} 张`;
  $('quiz-dir').textContent = c ? (c.direction ? '方向：' + c.direction : '反向卡') : '';
  $('quiz-prompt').textContent = c ? c.prompt : '没有待复习卡片';
  $('quiz-answer').hidden = true;
  $('quiz-answer').innerHTML = '';
  $('quiz-reveal').hidden = !c || Q.revealed;
  $('quiz-reveal').disabled = !c;
  $('quiz-grades').hidden = !c || !Q.revealed;
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
  $('quiz-note').textContent = '按 1-4 评分';
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
  const note = `已记录：${GRADE_LABEL[g]}`;
  if (Q.idx >= Q.cards.length) {
    await loadQuizCards();
    $('quiz-note').textContent = note + ' · 本轮完成';
  } else {
    $('quiz-note').textContent = note;
    renderQuiz();
  }
}

$('quiz-toggle').addEventListener('click', startQuiz);
$('quiz-exit').addEventListener('click', exitQuiz);
$('quiz-reveal').addEventListener('click', revealQuiz);
document.querySelectorAll('#quiz-grades button[data-grade]').forEach((b) => {
  b.addEventListener('click', () => gradeQuiz(Number(b.dataset.grade)));
});
// 深链：?quiz=1 直接进复习，便于分享与无头截图验证
{
  const qp = new URLSearchParams(location.search);
  if (qp.get('quiz') === '1') {
    startQuiz().then(() => { if (qp.get('reveal') === '1') revealQuiz(); });
  }
}
