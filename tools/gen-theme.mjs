// 从 design/tokens.json 生成两端主题产物（唯一真值来源）。
// 用法: node tools/gen-theme.mjs
// 产物: web/desktop.css 与 web/app.css 顶部的 CSS 变量块、
//       android/.../ThemeTokens.kt、android/.../res/values/colors.xml
// 幂等：重复运行只替换标记块之间的内容，不会重复插入。
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tokens = JSON.parse(fs.readFileSync(path.join(root, "design/tokens.json"), "utf8"));

const BEGIN = "/* === generated from design/tokens.json by tools/gen-theme.mjs - do not edit === */";
const END = "/* === end generated tokens === */";
const kebab = (s) => s.replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase();
const pascal = (s) => s[0].toUpperCase() + s.slice(1);
const snake = (s) => kebab(s).replace(/-/g, "_");
const hexArgb = (hex) => "#FF" + hex.slice(1).toUpperCase();
const hexCompose = (hex) => "0xFF" + hex.slice(1).toUpperCase();
function rgba(hex, a) {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}
for (const [name, scheme] of Object.entries(tokens.schemes)) {
  for (const [k, v] of Object.entries(scheme)) {
    if (!/^#[0-9a-fA-F]{6}$/.test(v)) throw new Error(`${name}.${k} 不是 #RRGGBB: ${v}`);
  }
}

// 旧变量名 -> M3 语义槽位。两个 CSS 文件历史上各有一套别名，统一都生成。
const ALIASES = {
  "--bg": "background",
  "--panel": "surfaceContainerLow",
  "--panel-alt": "surfaceContainer",
  "--panel2": "surfaceContainer",
  "--text": "onSurface",
  "--dim": "onSurfaceVariant",
  "--muted": "onSurfaceVariant",
  "--accent": "primary",
  "--line": "outlineVariant",
  "--divider": "outlineVariant",
  "--ok": "success",
  "--warn": "warning",
  "--bad": "error",
  "--danger": "error",
};

function schemeBlock(selector, s, indent = "") {
  const lines = [`${selector} {`];
  for (const [k, v] of Object.entries(s)) lines.push(`  --md-${kebab(k)}: ${v};`);
  lines.push("");
  lines.push("  /* 迁移期兼容别名：旧 CSS / Kotlin 仍可用 */");
  for (const [alias, slot] of Object.entries(ALIASES)) {
    lines.push(`  ${alias}: var(--md-${kebab(slot)});`);
  }
  lines.push("");
  lines.push("  /* 状态层（M3 state layers）：叠加在容器底色上 */");
  lines.push(`  --state-hover: ${rgba(s.onSurface, tokens.state.hover)};`);
  lines.push(`  --state-pressed: ${rgba(s.onSurface, tokens.state.pressed)};`);
  lines.push(`  --state-focus: ${rgba(s.primary, tokens.state.focus)};`);
  lines.push(`  --state-selected: ${rgba(s.primary, tokens.state.selected)};`);
  lines.push(`  --state-disabled-content: ${tokens.state.disabledContent};`);
  lines.push(`  --state-disabled-container: ${tokens.state.disabledContainer};`);
  lines.push("}");
  return lines.join("\n");
}

function cssBlock() {
  const d = tokens.density.comfortable;
  const c = tokens.density.compact;
  const lines = [BEGIN];
  lines.push(schemeBlock(":root", tokens.schemes.dark));
  lines.push("");
  lines.push("/* 亮色预留：设置 <body data-theme=\"light\"> 即整体切换 */");
  lines.push(schemeBlock('[data-theme="light"]', tokens.schemes.light));
  lines.push("");
  lines.push(":root {");
  for (const [k, v] of Object.entries(tokens.shape)) lines.push(`  --shape-${k}: ${v}px;`);
  for (const [k, v] of Object.entries(tokens.spacing)) lines.push(`  --space-${k}: ${v}px;`);
  for (const [k, v] of Object.entries(tokens.type)) {
    lines.push(`  --type-${kebab(k)}-size: ${v.size}px;`);
    lines.push(`  --type-${kebab(k)}-line: ${v.line}px;`);
  }
  for (const [k, v] of Object.entries(tokens.motion)) {
    lines.push(typeof v === "number" ? `  --motion-${kebab(k)}: ${v}ms;` : `  --motion-${kebab(k)}: ${v};`);
  }
  lines.push(`  --row-pad: ${d.rowPad}px; --pane-pad: ${d.panePad}px; --font-base: ${d.fontBase}px;`);
  lines.push("}");
  lines.push(`body[data-density="compact"] {`);
  lines.push(`  --row-pad: ${c.rowPad}px; --pane-pad: ${c.panePad}px; --font-base: ${c.fontBase}px;`);
  lines.push("}");
  lines.push(END);
  return lines.join("\n");
}

// 1) 两个 CSS 文件：替换标记块；首次运行先摘掉旧的 :root 变量块
const block = cssBlock();
for (const file of ["web/desktop.css", "web/app.css"]) {
  const p = path.join(root, file);
  let s = fs.readFileSync(p, "utf8");
  if (s.includes(BEGIN) && s.includes(END)) {
    const a = s.indexOf(BEGIN);
    const b = s.indexOf(END) + END.length;
    s = s.slice(0, a) + block + s.slice(b);
  } else {
    const at = s.indexOf(":root");
    if (at < 0) throw new Error(`${file} 找不到旧 :root 块`);
    const close = s.indexOf("}", at);
    s = s.slice(0, at) + s.slice(close + 1);
    s = block + "\n" + s.replace(/^\s*\n/, "");
  }
  fs.writeFileSync(p, s.replace(/\r\n/g, "\n"), "utf8");
  console.log(`  ${file}: tokens 已同步`);
}

// 2) Kotlin 常量（Compose 侧唯一色值来源）
//    Dark/Light 两套静态色 + MnemePalette 动态别名：14 个屏幕文件通过
//    MnemeColors 读动态值，切主题时无需逐个改文件。
const darkKeys = Object.keys(tokens.schemes.dark);
const kt = [
  "package io.github.morangsu.mneme", "",
  "import androidx.compose.runtime.mutableStateOf",
  "import androidx.compose.ui.graphics.Color",
  "import androidx.compose.ui.unit.dp", "",
  "/** 由 tools/gen-theme.mjs 从 design/tokens.json 生成，请勿手改。 */",
  "object MnemeTokens {",
  "    object Dark {",
];
for (const [k, v] of Object.entries(tokens.schemes.dark)) kt.push(`        val ${pascal(k)} = Color(${hexCompose(v)})`);
kt.push("    }", "", "    object Light {");
for (const [k, v] of Object.entries(tokens.schemes.light)) kt.push(`        val ${pascal(k)} = Color(${hexCompose(v)})`);
kt.push("    }", "", "    object Shape {");
for (const [k, v] of Object.entries(tokens.shape)) kt.push(`        val ${pascal(k)} = ${v}.dp`);
kt.push("    }", "", "    object Space {");
for (const [k, v] of Object.entries(tokens.spacing)) kt.push(`        val ${pascal(k)} = ${v}.dp`);
kt.push("    }", "", "    object Alpha {");
for (const [k, v] of Object.entries(tokens.state)) kt.push(`        const val ${pascal(k)} = ${v}f`);
kt.push("    }", "", "    object Type {");
for (const [k, v] of Object.entries(tokens.type)) {
  kt.push(`        const val ${pascal(k)}Size = ${v.size}`);
  kt.push(`        const val ${pascal(k)}Line = ${v.line}`);
}
kt.push("    }", "}", "",
  "/** 当前主题的动态色板；MnemeColors 与共用组件都读这里，切主题会整体重组。 */",
  "object MnemePalette {");
for (const k of darkKeys) {
  const P = pascal(k);
  kt.push(`    private val _${k} = mutableStateOf(MnemeTokens.Dark.${P})`);
  kt.push(`    var ${P}: Color`);
  kt.push(`        get() = _${k}.value`);
  kt.push(`        set(v) { _${k}.value = v }`);
}
kt.push("", "    fun apply(light: Boolean) {", "        if (light) {");
for (const k of darkKeys) kt.push(`            ${pascal(k)} = MnemeTokens.Light.${pascal(k)}`);
kt.push("        } else {");
for (const k of darkKeys) kt.push(`            ${pascal(k)} = MnemeTokens.Dark.${pascal(k)}`);
kt.push("        }", "    }", "}", "");
const ktPath = path.join(root, "android/app/src/main/java/io/github/morangsu/mneme/ThemeTokens.kt");
fs.writeFileSync(ktPath, kt.join("\n"), "utf8");
console.log("  android/.../ThemeTokens.kt: 已生成");

// 3) Android XML 资源：启动窗口/状态栏底色，旧名保留供 themes.xml 引用
const xml = ['<?xml version="1.0" encoding="utf-8"?>', "<!-- 由 tools/gen-theme.mjs 从 design/tokens.json 生成，请勿手改 -->", "<resources>"];
for (const [k, v] of Object.entries(tokens.schemes.dark)) xml.push(`    <color name="md_${snake(k)}">${hexArgb(v)}</color>`);
xml.push("", "    <!-- 兼容旧名：themes.xml 仍用 @color/bg 等 -->");
xml.push(`    <color name="bg">${hexArgb(tokens.schemes.dark.background)}</color>`);
xml.push(`    <color name="panel">${hexArgb(tokens.schemes.dark.surfaceContainerLow)}</color>`);
xml.push(`    <color name="panel_alt">${hexArgb(tokens.schemes.dark.surfaceContainer)}</color>`);
xml.push(`    <color name="text">${hexArgb(tokens.schemes.dark.onSurface)}</color>`);
xml.push(`    <color name="text_dim">${hexArgb(tokens.schemes.dark.onSurfaceVariant)}</color>`);
xml.push(`    <color name="accent">${hexArgb(tokens.schemes.dark.primary)}</color>`);
xml.push(`    <color name="divider">${hexArgb(tokens.schemes.dark.outlineVariant)}</color>`);
xml.push(`    <color name="danger">${hexArgb(tokens.schemes.dark.error)}</color>`);
xml.push(`    <color name="warn">${hexArgb(tokens.schemes.dark.warning)}</color>`);
xml.push(`    <color name="ok">${hexArgb(tokens.schemes.dark.success)}</color>`);
xml.push("</resources>", "");
const xmlPath = path.join(root, "android/app/src/main/res/values/colors.xml");
fs.writeFileSync(xmlPath, xml.join("\n"), "utf8");
console.log("  android/.../colors.xml: 已生成");
