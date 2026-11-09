#!/usr/bin/env node
/**
 * gh-commit.mjs — 不依赖 git 传输协议，用 GitHub REST API 把本地 git 索引推成一个提交。
 *
 * 为什么需要它：本机到 github.com 的 443 被阻断（git 的 smart-HTTP 走不通），
 * 但 api.github.com 可达。Git Data API（blobs → tree → commit → ref）可以只用一个
 * 提交把所有文件写上去，且提交历史干净（不是每文件一个 commit 的 Contents API 做法）。
 *
 * 用法：
 *   GH_TOKEN=xxx node tools/gh-commit.mjs <repoDir> <owner/name> [message]
 *
 * 文件清单取自 `git ls-files`，因此 .gitignore 仍然生效；工作区内容为准（不是索引里的旧 blob）。
 */

import { execFileSync } from "node:child_process";
import path from "node:path";

const API = "https://api.github.com";
const TOKEN = process.env.GH_TOKEN;
if (!TOKEN) {
  console.error("缺少 GH_TOKEN 环境变量");
  process.exit(1);
}

const [repoDir, slug, message = "chore: sync"] = process.argv.slice(2);
if (!repoDir || !slug || !slug.includes("/")) {
  console.error("用法: GH_TOKEN=xxx node tools/gh-commit.mjs <repoDir> <owner/name> [message]");
  process.exit(1);
}

const root = path.resolve(repoDir);

async function api(method, url, body) {
  for (let attempt = 1; attempt <= 4; attempt++) {
    const res = await fetch(url.startsWith("http") ? url : API + url, {
      method,
      headers: {
        Authorization: `Bearer ${TOKEN}`,
        Accept: "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        ...(body ? { "Content-Type": "application/json" } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    let data;
    try {
      data = text ? JSON.parse(text) : {};
    } catch {
      data = { raw: text };
    }
    if (res.ok) return { status: res.status, data };
    // 409/422 是业务结论，不重试
    if (res.status >= 400 && res.status < 500 && res.status !== 429) {
      return { status: res.status, data };
    }
    if (attempt === 4) return { status: res.status, data };
    const wait = 500 * 2 ** (attempt - 1);
    console.error(`  ! ${method} ${url} -> ${res.status}，${wait}ms 后重试`);
    await new Promise((r) => setTimeout(r, wait));
  }
}

function listTracked(dir) {
  const out = execFileSync("git", ["ls-files", "-s", "-z"], { cwd: dir, maxBuffer: 1 << 28 });
  return out
    .toString("utf8")
    .split("\0")
    .filter(Boolean)
    .map((rec) => {
      // 格式: "<mode> <object> <stage>\t<path>"
      const tab = rec.indexOf("\t");
      const head = rec.slice(0, tab).split(" ");
      return {
        mode: head[0] === "100755" ? "100755" : "100644",
        sha: head[1],
        path: rec.slice(tab + 1),
      };
    });
}

// 读索引里的 blob，而不是工作区文件：这样 eol / 清理规则已经由 git 应用过，
// 推上去的内容和 `git push` 完全一致（Windows 上工作区是 CRLF，仓库里是 LF）。
function readBlob(dir, sha) {
  return execFileSync("git", ["cat-file", "blob", sha], {
    cwd: dir,
    maxBuffer: 1 << 28,
  });
}

const files = listTracked(root);
if (files.length === 0) {
  console.error("git 索引里没有文件，先 git add");
  process.exit(1);
}
console.log(`仓库 ${slug}：${files.length} 个文件，根目录 ${root}`);

// 0) 空仓库没法直接建 blob（GitHub 返回 409 "Git Repository is empty"），
//    先用 Contents API 造一个引导提交，之后的 Git Data API 才有 parent 可挂。
const probeRef = await api("GET", `/repos/${slug}/git/ref/heads/main`);
if (probeRef.status !== 200) {
  const boot = await api("PUT", `/repos/${slug}/contents/.gitkeep`, {
    message: "chore: 初始化仓库",
    content: Buffer.from("").toString("base64"),
  });
  if (boot.status !== 201 && boot.status !== 200) {
    console.error(`引导提交失败: ${boot.status} ${JSON.stringify(boot.data)}`);
    process.exit(1);
  }
  console.log(`  引导提交 ${boot.data.commit.sha.slice(0, 10)}（空仓库必需）`);
}

// 1) blobs（并发 6，避免触发二级限流）
const CONCURRENCY = 6;
const treeEntries = [];
let done = 0;
async function worker(queue) {
  while (queue.length) {
    const entry = queue.shift();
    const p = entry.path;
    const buf = readBlob(root, entry.sha);
    const r = await api("POST", `/repos/${slug}/git/blobs`, {
      content: buf.toString("base64"),
      encoding: "base64",
    });
    if (r.status !== 201) {
      console.error(`\nblob 失败 ${p}: ${r.status} ${JSON.stringify(r.data)}`);
      process.exit(1);
    }
    const mode = entry.mode;
    treeEntries.push({ path: p, mode, type: "blob", sha: r.data.sha });
    done++;
    if (done % 20 === 0 || done === files.length) process.stdout.write(`\r  blobs ${done}/${files.length}`);
  }
}
const queue = [...files];
await Promise.all(Array.from({ length: CONCURRENCY }, () => worker(queue)));
console.log("");

// 2) tree
const treeRes = await api("POST", `/repos/${slug}/git/trees`, { tree: treeEntries });
if (treeRes.status !== 201) {
  console.error(`tree 失败: ${treeRes.status} ${JSON.stringify(treeRes.data)}`);
  process.exit(1);
}
const treeSha = treeRes.data.sha;
console.log(`  tree ${treeSha.slice(0, 10)}`);

// 3) 父提交（空仓库时不存在）
const refRes = await api("GET", `/repos/${slug}/git/ref/heads/main`);
const parents = refRes.status === 200 ? [refRes.data.object.sha] : [];
if (refRes.status !== 200) console.log("  （空仓库，首个提交）");

const commitRes = await api("POST", `/repos/${slug}/git/commits`, {
  message,
  tree: treeSha,
  parents,
});
if (commitRes.status !== 201) {
  console.error(`commit 失败: ${commitRes.status} ${JSON.stringify(commitRes.data)}`);
  process.exit(1);
}
const commitSha = commitRes.data.sha;
console.log(`  commit ${commitSha.slice(0, 10)}  ${message}`);

// 4) ref
const upd = parents.length
  ? await api("PATCH", `/repos/${slug}/git/refs/heads/main`, { sha: commitSha, force: false })
  : await api("POST", `/repos/${slug}/git/refs`, { ref: "refs/heads/main", sha: commitSha });

if (upd.status !== 200 && upd.status !== 201) {
  console.error(`ref 更新失败: ${upd.status} ${JSON.stringify(upd.data)}`);
  process.exit(1);
}
console.log(`✓ https://github.com/${slug}/commit/${commitSha}`);
