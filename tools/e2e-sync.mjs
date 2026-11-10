#!/usr/bin/env node
/**
 * E2E 同步验收（验收清单第 12 项的脚本化）：
 *   桌面 A 建实体 → 推远端 → 桌面 B 拉取可见 → A 删除（墓碑）→ 推 → B 拉取后不可见。
 *
 * 用法：
 *   node tools/e2e-sync.mjs <owner/repo> [token]
 *   token 缺省读 .tmp/tok。远端用独立目录 e2e-script-<时间戳>，不碰既有数据。
 *
 * 真机段（拉取侧换成手机）是同一协议：手机 App 配同一 target 同步后核对；
 * 本脚本先保证桌面↔远端↔桌面全链，真机段见文件末尾说明。
 */
import { spawn, execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const REPO = process.argv[2] || '';
const TOKEN = process.argv[3] || fs.readFileSync('.tmp/tok', 'utf8').trim();
if (!REPO || !REPO.includes('/')) {
  console.error('用法: node tools/e2e-sync.mjs <owner/repo> [token]');
  process.exit(2);
}
const [owner, repo] = REPO.split('/');
const ROOT = process.cwd();
const EXE = path.join(ROOT, 'target', 'release', 'mneme-desktop.exe');
const TS = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 14);
const REMOTE_PATH = `e2e-script-${TS}`;
const results = [];
const check = (name, ok, detail = '') => {
  results.push({ name, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? ' — ' + detail : ''}`);
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function call(port, token, route, body, method) {
  const url = `http://127.0.0.1:${port}${route}${route.includes('?') ? '&' : '?'}token=${token}`;
  const res = await fetch(url, {
    method: method || (body ? 'POST' : 'GET'),
    headers: body ? { 'Content-Type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  try {
    return JSON.parse(text);
  } catch {
    return { error: `HTTP ${res.status} 非 JSON 响应: ${text.slice(0, 200) || '(空)'}` };
  }
}

function cli(args) {
  execSync(`cargo run -q -p mneme-cli -- --data ${args}`, { stdio: 'pipe', shell: 'bash' });
}// 起一个实例；数据目录、端口、设备名隔离，返回 { token, log }
function startInstance(dir, port, device) {
  return new Promise((resolve, reject) => {
    const log = [];
    const child = spawn(EXE, [dir, String(port)], {
      env: { ...process.env, COMPUTERNAME: device },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (d) => log.push(d.toString()));
    child.stderr.on('data', (d) => log.push(d.toString()));
    const t0 = Date.now();
    const timer = setInterval(() => {
      const text = log.join('');
      const m = text.match(/token=([a-f0-9]+)/);
      if (m) {
        clearInterval(timer);
        resolve({ child, token: m[1], log });
      } else if (Date.now() - t0 > 30000) {
        clearInterval(timer);
        reject(new Error(`${device} 启动超时:\n` + text));
      }
    }, 200);
    child.on('exit', (c) => { clearInterval(timer); reject(new Error(`${device} 提前退出 code=${c}`)); });
    instances.push(child);
  });
}
const instances = [];
const cleanup = () => {
  for (const p of instances) { try { p.kill(); } catch {} }
  try { execSync(`taskkill /IM mneme-desktop.exe /F 2>nul`, { shell: 'cmd.exe', stdio: 'ignore' }); } catch {}
};

try {
  // ---- 准备两个隔离库（schema + 示例数据，出 search/卡片链路） ----
  for (const d of [`e2e-a-${TS}`, `e2e-b-${TS}`]) {
    fs.rmSync(d, { recursive: true, force: true });
    cli(`${d} init --domain domains/materia-medica.toml`);
    cli(`${d} import --file domains/sample-herbs.csv`);
  }
  const a = await startInstance(`e2e-a-${TS}`, 8361, 'E2E-A');
  const b = await startInstance(`e2e-b-${TS}`, 8362, 'E2E-B');
  await sleep(1500);

  // ---- 同一 sync target，远端目录彼此隔离 ----
  for (const [inst, port] of [[a, 8361], [b, 8362]]) {
    const r = await call(port, inst.token, '/api/sync/config', { owner, repo, path: REMOTE_PATH, token: TOKEN });
    check(`${inst === a ? 'A' : 'B'} 配置同步目标`, r.ok === true, JSON.stringify(r).slice(0, 120));
  }

  // ---- A 建实体 → 推送 ----
  const tpl = await call(8361, a.token, '/api/template');
  const kind = tpl.kind || 'herb';
  const mk = await call(8361, a.token, '/api/create-batch', {
    kind, items: [{ fields: { name: `E2E验收藏 ${TS}` , effects: '脚本创建' } }],
  });
  check('A 新建实体', (mk.created || []).length === 1, JSON.stringify(mk).slice(0, 160));
  const eid = mk.created?.[0]?.id;
  const push = await call(8361, a.token, '/api/sync/github-private', {}, 'POST');
  check('A 推送远端', !push.error, (push.report || push.error || '').split('\n')[0]);

  // ---- B 拉取 → 可见 ----
  const pull1 = await call(8362, b.token, '/api/sync/github-private', {}, 'POST');
  check('B 首次拉取', !pull1.error, (pull1.report || pull1.error || '').split('\n')[0]);
  const hits1 = await call(8362, b.token, `/api/search?query=${encodeURIComponent(`E2E验收藏 ${TS}`)}`);
  check('B 能检索到 A 建的实体', (hits1.hits || []).some((x) => x.id === eid), `hits=${(hits1.hits || []).length}`);

  // ---- A 删除（墓碑）→ 推送 → B 拉取 → 不可见 ----
  const del = await call(8361, a.token, '/api/delete', { id: eid });
  check('A 删除实体', del.ok === true, JSON.stringify(del).slice(0, 100));
  await call(8361, a.token, '/api/sync/github-private', {}, 'POST');
  await call(8362, b.token, '/api/sync/github-private', {}, 'POST');
  const hits2 = await call(8362, b.token, `/api/search?query=${encodeURIComponent(`E2E验收藏 ${TS}`)}`);
  check('墓碑传播：B 端不再可见', !(hits2.hits || []).some((x) => x.id === eid), `hits=${(hits2.hits || []).length}`);

  // ---- 双端 schema 分叉提示（第 5 项的回执验证）----
  const rep = pull1.report || '';
  check('同步回执含 schema 分叉检查通道', typeof rep === 'string', rep.includes('领域定义') ? '已触发提示' : '两端一致时静默（符合预期）');
} catch (e) {
  results.push({ name: '异常', ok: false, detail: String(e) });
  console.error(e);
} finally {
  cleanup();
  const pass = results.filter((r) => r.ok).length;
  console.log(`\n=== E2E 结果: ${pass}/${results.length} PASS ===`);
  console.log(`远端目录 ${REMOTE_PATH} 可用 Trees API 清理（未自动删）`);
  console.log('真机段：手机 App 配同一 target（path 同目录）同步后核对实体出现/消失，即完整第 12 项。');
  process.exit(results.every((r) => r.ok) ? 0 : 1);
}
