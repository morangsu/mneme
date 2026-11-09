const $ = (id) => document.getElementById(id);
const TOKEN = new URLSearchParams(location.search).get('token') || '';
let currentCard = null;

function authHeaders(extra) {
  const h = Object.assign({}, extra || {});
  if (TOKEN) h['Authorization'] = 'Bearer ' + TOKEN;
  return h;
}

async function api(path, opt = {}) {
  opt.headers = authHeaders(opt.headers);
  const r = await fetch(path, opt);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return { error: t }; }
}
const esc = (s) => String(s ?? '').replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));

function banner(msg, kind = 'warn') {
  $('banners').innerHTML = `<div class="banner ${kind}">${esc(msg)}</div>`;
}

async function boot() {
  const s = await api('/api/state');
  if (s.error) { banner(s.error, 'bad'); return; }
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
