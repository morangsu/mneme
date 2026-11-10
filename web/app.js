const $ = (id) => document.getElementById(id);
let currentCard = null;

// 登录态在 HttpOnly 会话 cookie 里（/api/login 签发），JS 不碰 token 本体——
// localStorage 里存 token 等于给 XSS 留一把长期钥匙。
// 例外：URL 带 ?token= 时仍向后传（桌面壳与脚本调试用这个通道）。
const URL_TOKEN = new URLSearchParams(location.search).get('token') || '';

function authHeaders(extra) {
  const h = Object.assign({}, extra || {});
  if (URL_TOKEN) h['Authorization'] = 'Bearer ' + URL_TOKEN;
  return h;
}

async function api(path, opt = {}) {
  opt.headers = authHeaders(opt.headers);
  const r = await fetch(path, opt);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return { error: t }; }
}

// 401 = 没登录或会话过期：弹登录框，登成功刷新整页
async function apiAuthed(path, opt = {}) {
  const r = await api(path, opt);
  if (r.error && /需要登录|401/i.test(r.error)) { showLogin(); }
  return r;
}

function showLogin() {
  if ($('login-mask')) return;
  const mask = document.createElement('div');
  mask.id = 'login-mask';
  mask.style.cssText =
    'position:fixed;inset:0;background:rgba(0,0,0,.82);display:flex;' +
    'align-items:center;justify-content:center;z-index:99';
  mask.innerHTML = `
    <form id="login-form" style="background:var(--md-surface-container-low);padding:var(--space-xl);border-radius:var(--shape-l);width:min(92vw,360px);display:flex;flex-direction:column;gap:12px">
      <b style="font-size:16px">mneme 登录</b>
      <span class="muted">输入访问口令（同一次输入保持 12 小时）</span>
      <input type="password" id="login-token" autocomplete="current-password"
             style="padding:var(--space-m);border-radius:var(--shape-s);border:1px solid var(--md-outline);background:var(--md-surface-container-lowest);color:var(--md-on-surface)" />
      <div id="login-err" class="muted" style="color:var(--md-error)"></div>
      <button type="submit" style="padding:var(--space-m);border-radius:var(--shape-s);border:none;background:var(--md-primary);color:var(--md-on-primary);font-weight:500">登录</button>
    </form>`;
  document.body.appendChild(mask);
  $('login-token').focus();
  $('login-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const r = await api('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: $('login-token').value }),
    });
    if (r.error) { $('login-err').textContent = r.error; return; }
    mask.remove();
    location.reload();
  });
}
const esc = (s) => String(s ?? '').replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));

function banner(msg, kind = 'warn') {
  $('banners').innerHTML = `<div class="banner ${kind}">${esc(msg)}</div>`;
}

async function boot() {
  const s = await api('/api/state');
  if (s.error) {
    if (/需要登录|401/i.test(s.error)) { showLogin(); }
    else banner(s.error, 'bad');
    return;
  }
  $('state').textContent = `${s.schema || '未加载领域'} · ${s.entities} 条 · ${s.cards} 张卡 · ${s.device}`;
  const v = await api('/api/views');
  $('view').innerHTML = (v.views || []).map(x => `<option value="${esc(x.id)}">${esc(x.label || x.id)}</option>`).join('');
  if ((v.views || []).length) await loadView();
  const t = await api('/api/sync/status');
  $('targets').innerHTML = (t.targets || []).length
    ? t.targets.map(x => `<button onclick="sync('${esc(x)}',false)">同步 ${esc(x)}</button><button onclick="sync('${esc(x)}',true)">预演</button>`).join(' ')
    : '<span class="muted">未配置同步目标（data/sync.json）</span>';
  await nextCard();
}

$('view').addEventListener('change', loadView);

async function loadView() {
  const id = $('view').value;
  if (!id) return;
  const v = await api('/api/view/' + encodeURIComponent(id));
  if (v.error) { $('table').textContent = v.error; return; }
  $('view-note').textContent = v.hidden?.length ? `出题视图隐藏列：${v.hidden.join('、')}` : `${v.rows.length} 行`;
  const head = `<tr>${v.columns.map(c => `<th>${esc(c.label || c.key)}</th>`).join('')}</tr>`;
  let lastGroup = null;
  const body = v.rows.map(r => {
    let groupRow = '';
    if (v.group_by && r._group !== lastGroup) {
      lastGroup = r._group;
      groupRow = `<tr class="group-row"><td colspan="${v.columns.length}">${esc(r._group || '未分组')}</td></tr>`;
    }
    const cells = v.columns.map(c => {
      const val = r[c.key] || '';
      if (c.key === v.primary) return `<td class="name">${esc(val)}</td>`;
      if (c.immutable) return `<td><span class="tag warn">原样</span>${esc(val)}</td>`;
      return `<td>${esc(val)}</td>`;
    }).join('');
    return groupRow + `<tr>${cells}</tr>`;
  }).join('');
  $('table').innerHTML = `<table>${head}<tbody>${body || `<tr><td colspan="${v.columns.length}">没有数据</td></tr>`}</tbody></table>`;
}

async function doReverse() {
  const raw = $('clue').value.trim();
  if (!raw) return;
  const clues = raw.split(/\s+/).map(s => s.replace(/：/g, ':'));
  const r = await api('/api/reverse?clue=' + encodeURIComponent(clues.join('|')));
  if (r.error) { $('reverse-out').innerHTML = `<div class="banner bad">${esc(r.error)}</div>`; return; }
  $('reverse-out').innerHTML = (r.items || []).length
    ? `<div class="panel"><h2>${r.items.length} 个候选</h2>${r.items.map(x =>
        `<div class="card"><b>${esc(x.name)}</b> <span class="tag">${esc(x.group || '')}</span><div class="muted">${esc(x.summary || '')}</div></div>`).join('')}</div>`
    : '<div class="banner">没有命中；检查字段名（功效/应用/特点/禁忌）或换更短的关键词。</div>';
}

$('clue').addEventListener('keydown', (e) => { if (e.key === 'Enter') doReverse(); });

async function nextCard() {
  const r = await api('/api/quiz?limit=1');
  currentCard = (r.cards || [])[0] || null;
  $('q').textContent = currentCard ? currentCard.prompt : '（还没有卡片，先导入数据）';
  $('a').style.display = 'none';
  $('quiz-stat').textContent = r.due != null ? `到期 ${r.due} 张` : '';
}

async function reveal() {
  if (!currentCard) return;
  const r = await api('/api/quiz/answer/' + encodeURIComponent(currentCard.id));
  $('a').style.display = 'block';
  $('a').textContent = r.answer + (r.boundary?.length ? `\n\n注意：${r.boundary.join('；')}` : '');
}

async function judgeAnswer() {
  if (!currentCard) return;
  const r = await api('/api/judge', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ card_id: currentCard.id, input: $('answer-input').value })
  });
  $('judge-out').textContent = r.error
    ? r.error
    : `${r.correct ? '正确' : '再想想'} · 相似度 ${Number(r.score).toFixed(2)} · ${r.matched || ''}`;
}

$('answer-input').addEventListener('keydown', (e) => { if (e.key === 'Enter') judgeAnswer(); });

async function grade(g) {
  if (!currentCard) return;
  await api('/api/review', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ card_id: currentCard.id, grade: g })
  });
  await nextCard();
}

async function download() {
  const f = $('fmt').value;
  const r = await fetch('/api/export?format=' + f, { headers: authHeaders() });
  if (!r.ok) { $('io-out').textContent = '导出失败: ' + await r.text(); return; }
  const b = await r.blob();
  const a = document.createElement('a');
  a.href = URL.createObjectURL(b);
  a.download = 'mneme-' + f + '.' + (f === 'md' ? 'md' : f === 'html' ? 'html' : f === 'anki' ? 'txt' : f);
  a.click();
  $('io-out').textContent = `已导出 ${b.size} 字节`;
}

async function upload() {
  const f = $('file').files[0];
  if (!f) { $('io-out').textContent = '请先选择文件'; return; }
  const text = await f.text();
  const r = await api('/api/import', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ content: text, filename: f.name, dry: $('dry').checked })
  });
  $('io-out').textContent = JSON.stringify(r, null, 2);
  await boot();
}

async function sync(id, dry) {
  $('sync-out').textContent = '同步中…';
  const r = await api(`/api/sync/${encodeURIComponent(id)}?dry=${dry}`, { method: 'POST' });
  $('sync-out').textContent = r.report || JSON.stringify(r, null, 2);
}

document.querySelectorAll('header nav a').forEach(a => {
  a.addEventListener('click', (e) => {
    e.preventDefault();
    document.querySelectorAll('header nav a').forEach(x => x.classList.remove('on'));
    a.classList.add('on');
    ['matrix', 'reverse', 'quiz', 'data'].forEach(t => { $('tab-' + t).hidden = t !== a.dataset.tab; });
  });
});

boot();

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
