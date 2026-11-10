//! mneme 桌面壳。
//!
//! 选型结论（见 docs/DESKTOP-BASELINE.md）：**不打包浏览器引擎**。
//! Windows 用系统预装的 WebView2（随 Edge 分发），Linux 用 WebKitGTK，macOS 用 WKWebView；
//! 安装包里只有 Rust 二进制 + 前端资源。UI 复用 `mneme-server` 内嵌的那套网页
//! （同一个 `router()`），桌面端与手机浏览器看到的是同一份前端、同一个 API——
//! 业务规则只有一份实现，不存在「两端口径分叉」。
//!
//! 为什么是 `wry` 而不是 Tauri：Tauri 的 ACL/IPC 对「纯本地 HTTP」是绕路，
//! wry 是最薄的一层——开窗 + 加载 URL，其余交给同进程的 axum。
//!
//! 启动时间线打在 stdout，作为桌面端首帧的回归基线：
//! 进程起点 → 库打开（含 OpenTimings）→ 服务监听 → WebView 建好 → 页面加载完成。

use anyhow::Context;
use mneme_server::AppState;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    // 壳没有可恢复的启动失败：WebView2 缺失、端口被占这类错误直接带着原因退出
    if let Err(e) = run() {
        eprintln!("mneme 启动失败：{e:#}");
        std::process::exit(1);
    }
}

#[allow(unreachable_code)]
fn run() -> anyhow::Result<()> {
    let t0 = Instant::now();
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(args.next().unwrap_or_else(|| "data".to_string()));
    let fixed_port: Option<u16> = args.next().and_then(|s| s.parse().ok());
    let device = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "desktop".into());
    // 剪贴板识字要拿库根目录找模型；data 马上会被 move 进后台线程，这里先留一份
    let ocr_root = data.clone();

    // 库打开与 WebView 创建**并行**：WebView2 冷建 1.7 秒，而库只要几十毫秒，
    // 串行就是白白排队。主线程立刻开窗显示「正在打开」，后台线程备好 URL 后再导航。
    let (url_tx, url_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let r = (|| -> anyhow::Result<String> {
            let (store, timings) = mneme_store::Store::open_timed(&data, &device)?;
            println!(
                "[t= {:>7.1} ms] 库已打开（重放 {:.1} / 索引 {:.1} / 卡片 {:.1} / 镜像 {:.1}，{} 实体）",
                t0.elapsed().as_secs_f64() * 1000.0,
                timings.replay.as_secs_f64() * 1000.0,
                timings.derive.as_secs_f64() * 1000.0,
                timings.cards_cache.as_secs_f64() * 1000.0,
                timings.sqlite_rebuild.as_secs_f64() * 1000.0,
                store.stats().entities,
            );
            // 同进程 axum：随机端口 + 每次启动都变的随机 token，仅听 127.0.0.1。
            // token 挡住本机其他进程；`/desktop` 页面本身公开，数据都在带 token 的 API 后面。
            // 有 sync.json 就直接建同步引擎（桌面端打开即可同步，不用先去设置页）
            let sync_engine = mneme_server::load_sync_config(
                &store,
                &store.root().join("sync.json"),
            );
            let token = local_token();
            let state = AppState::new(store, sync_engine, Some(token.clone()));
            let server = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], fixed_port.unwrap_or(0)));
            let listener = server
                .block_on(async { tokio::net::TcpListener::bind(addr).await })
                .context("绑定 127.0.0.1 失败")?;
            let port = listener.local_addr().context("local_addr")?.port();
            std::thread::spawn(move || {
                server.block_on(async {
                    if let Err(e) = mneme_server::serve_on(state, listener).await {
                        eprintln!("server 退出: {e}");
                    }
                });
            });
            println!(
                "[t= {:>7.1} ms] 服务已监听 http://127.0.0.1:{port}/desktop?token={token}",
                t0.elapsed().as_secs_f64() * 1000.0
            );
            Ok(format!("http://127.0.0.1:{port}/desktop?token={token}"))
        })();
        let _ = url_tx.send(r.unwrap_or_else(|e| format!("about:blank#error={}", urlencoding_lite(&e.to_string()))));
    });

    // 3) 主线程：立刻开窗 + WebView（先显示占位页），URL 就绪后导航过去。
    use tao::event_loop::{ControlFlow, EventLoop};
    use tao::window::WindowBuilder;
    use wry::{PageLoadEvent, WebViewBuilder};

    const LOADING: &str = concat!(
        "<body style=\"margin:0;background:#16171a;color:#e6e7ea;",
        "font-family:'Microsoft YaHei',sans-serif;display:flex;height:100vh;",
        "align-items:center;justify-content:center;flex-direction:column;gap:8px\">",
        "<div style=\"font-size:20px;font-weight:600\">mneme</div>",
        "<div style=\"color:#9a9ca3;font-size:13px\">正在打开库…</div></body>"
    );

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("mneme")
        .with_inner_size(tao::dpi::LogicalSize::new(1180.0, 780.0))
        .build(&event_loop)?;
    let webview = WebViewBuilder::new()
        .with_html(LOADING)
        .with_on_page_load_handler(move |event, _| {
            if matches!(event, PageLoadEvent::Finished) {
                println!(
                    "[t= {:>7.1} ms] 页面加载完成",
                    t0.elapsed().as_secs_f64() * 1000.0
                );
            }
        })
        .build(&window)
        .context("创建 WebView 失败（需要 WebView2 Runtime）")?;
    println!(
        "[t= {:>7.1} ms] WebView 已创建（窗口先出，数据后台备）",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // 4) 托盘：关窗=隐藏（进程和数据都活着），点托盘或菜单「显示」唤起。
    //     第二次打开因此是瞬时的——WebView、库、server 全部还在。
    let tray = build_tray();
    let (menu_tx, menu_rx) = std::sync::mpsc::channel::<TrayAction>();
    tray_icon::TrayIconEvent::set_event_handler(Some(move |e| {
        // 左键点图标本体 = 显示窗口
        if matches!(e, tray_icon::TrayIconEvent::Click { button: tray_icon::MouseButton::Left, button_state: tray_icon::MouseButtonState::Up, .. }) {
            let _ = menu_tx.send(TrayAction::Show);
        }
    }));
    let (item_tx, item_rx) = std::sync::mpsc::channel::<TrayAction>();
    {
        let item_tx = item_tx.clone();
        tray_icon::menu::MenuEvent::set_event_handler(Some(move |e: tray_icon::menu::MenuEvent| {
            match e.id().as_ref() {
                SHOW_ID => { let _ = item_tx.send(TrayAction::Show); }
                OCR_ID => { let _ = item_tx.send(TrayAction::OcrClipboard); }
                QUIT_ID => { let _ = item_tx.send(TrayAction::Quit); }
                _ => {}
            }
        }));
    }

    // 5) 全局热键 Ctrl+Alt+O：任何应用里截图（Win+Shift+S）后一按，
    //    剪贴板里的图就变成本地 OCR 文本回到 mneme 的识字面板。
    let hotkey_mgr = global_hotkey::GlobalHotKeyManager::new()
        .map_err(|e| anyhow::anyhow!("全局热键管理器创建失败: {e}"))?;
    let hotkey = global_hotkey::hotkey::HotKey::new(
        Some(global_hotkey::hotkey::Modifiers::CONTROL | global_hotkey::hotkey::Modifiers::ALT),
        global_hotkey::hotkey::Code::KeyO,
    );
    // 注册冲突（别的软件占了）不致命：托盘菜单里的「识字剪贴板」还能用。
    if let Err(e) = hotkey_mgr.register(hotkey) {
        eprintln!("Ctrl+Alt+O 注册失败（可能被其他软件占用，托盘菜单仍可用）: {e}");
    }
    {
        let tx = item_tx.clone();
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(move |e: global_hotkey::GlobalHotKeyEvent| {
            if e.state == global_hotkey::HotKeyState::Pressed {
                let _ = tx.send(TrayAction::OcrClipboard);
            }
        }));
    }

    let mut pending = Some(url_rx);
    event_loop.run(move |event, _, control_flow| {
        match event {
            tao::event::Event::NewEvents(..) => {
                // URL 就绪（或失败）就导航过去；就绪前保持轮询
                if let Some(rx) = &pending {
                    if let Ok(url) = rx.try_recv() {
                        let _ = webview.load_url(&url);
                        pending = None;
                    }
                }
                // 托盘/热键事件（几个 handler 分开，这里统一收）
                for rx in [&menu_rx, &item_rx] {
                    while let Ok(action) = rx.try_recv() {
                        match action {
                            TrayAction::Show => window.set_visible(true),
                            TrayAction::OcrClipboard => {
                                spawn_clipboard_ocr(&ocr_root, &item_tx);
                            }
                            TrayAction::OcrDone(text) => {
                                window.set_visible(true);
                                let js = serde_json::to_string(&text)
                                    .unwrap_or_else(|_| "\"\"".into());
                                let _ = webview.evaluate_script(&format!(
                                    "window.__mnemeOcrText && __mnemeOcrText({js})"
                                ));
                            }
                            TrayAction::Quit => *control_flow = ControlFlow::Exit,
                        }
                    }
                }
                let mut any_pending = pending.is_some();
                for rx in [&menu_rx, &item_rx] {
                    if !rx.try_recv().is_err() {
                        any_pending = true;
                    }
                }
                *control_flow = if pending.is_some() || any_pending {
                    ControlFlow::Poll
                } else {
                    ControlFlow::Wait
                };
            }
            tao::event::Event::WindowEvent {
                event: tao::event::WindowEvent::CloseRequested,
                ..
            } => {
                // 关窗=隐藏到托盘：进程与数据常驻，再次打开瞬时可达。
                // 真正退出走托盘菜单「退出」。
                window.set_visible(false);
            }
            _ => {}
        }
        let _ = (&webview, &tray); // 保持 WebView 与托盘存活
    });
    Ok(())
}

enum TrayAction {
    Show,
    /// 读剪贴板图片 → 本地 OCR → 结果回主窗（热键与托盘菜单共用）
    OcrClipboard,
    /// OCR 完成，带识别文本（由 OCR 线程发出）
    OcrDone(String),
    Quit,
}

const SHOW_ID: &str = "mneme-show";
const OCR_ID: &str = "mneme-ocr";
const QUIT_ID: &str = "mneme-quit";

/// 剪贴板识字：读图（失败静默提示进托盘 tooltip 不够醒目，就交给 stderr 与主窗）
/// → PNG 编码 → 后台线程 OCR → `OcrDone(文本)` 发回事件循环。
///
/// 在事件循环线程调用（剪贴板要尽快开快关，避免锁住别的应用的剪贴板）。
fn spawn_clipboard_ocr(root: &std::path::Path, tx: &std::sync::mpsc::Sender<TrayAction>) {
    use std::io::Cursor;
    let tx = tx.clone();
    let img = (|| -> anyhow::Result<image::RgbaImage> {
        let mut clip = arboard::Clipboard::new().context("打开剪贴板失败")?;
        let data = clip.get_image().context("剪贴板里没有图片（先用 Win+Shift+S 截图）")?;
        let buf = data.bytes.into_owned();
        image::RgbaImage::from_raw(data.width as u32, data.height as u32, buf)
            .context("剪贴板图片数据不完整")
    })();
    let img = match img {
        Ok(i) => i,
        Err(e) => {
            eprintln!("识字剪贴板：{e:#}");
            return;
        }
    };
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        let run = (|| -> anyhow::Result<String> {
            let mut png = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .context("截图编码失败")?;
            let out = mneme_server::ocr::ocr_bytes_blocking(&root, &png, false)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(out.text)
        })();
        match run {
            Ok(text) => { let _ = tx.send(TrayAction::OcrDone(text)); }
            Err(e) => {
                eprintln!("识字剪贴板失败：{e:#}");
                let _ = tx.send(TrayAction::OcrDone(format!("（识字失败：{e:#}）")));
            }
        }
    });
}

/// 托盘图标是程序里现画的像素「m」——不想为了一个图标引入资源文件与构建步骤。
fn build_tray() -> tray_icon::TrayIcon {
    use tray_icon::menu::{Menu, MenuItem};
    use tray_icon::{TrayIconBuilder};

    const W: usize = 24;
    // 简单的像素字母 m（accent 色块），1 = 实心
    const ART: [&str; 8] = [
        "..####..",
        ".######.",
        "##.##.##",
        "##.##.##",
        "##.##.##",
        "##.##.##",
        "##...###",
        "##...###",
    ];
    let mut rgba = vec![0u8; W * W * 4];
    let scale = 2usize; // 8x8 艺术放大到 16x16，居中在 24x24 里
    let ox = (W - 8 * scale) / 2;
    let oy = (W - 8 * scale) / 2;
    for (r, line) in ART.iter().enumerate() {
        for (c, ch) in line.chars().enumerate() {
            if ch != '#' {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let x = ox + c * scale + dx;
                    let y = oy + r * scale + dy;
                    let i = (y * W + x) * 4;
                    // #6aa9ff
                    rgba[i] = 0x6a;
                    rgba[i + 1] = 0xa9;
                    rgba[i + 2] = 0xff;
                    rgba[i + 3] = 0xff;
                }
            }
        }
    }
    let icon = tray_icon::Icon::from_rgba(rgba, W as u32, W as u32).expect("托盘图标");

    let menu = Menu::new();
    let show = MenuItem::with_id(SHOW_ID, "显示 mneme", true, None);
    let ocr = MenuItem::with_id(OCR_ID, "识字剪贴板 (Ctrl+Alt+O)", true, None);
    let quit = MenuItem::with_id(QUIT_ID, "退出", true, None);
    let _ = menu.append(&show);
    let _ = menu.append(&ocr);
    let _ = menu.append(&quit);

    TrayIconBuilder::new()
        .with_id("mneme-tray")
        .with_icon(icon)
        .with_tooltip("mneme — 关闭窗口后仍在此处")
        .with_menu(Box::new(menu))
        .build()
        .expect("创建托盘失败")
}

/// 错误信息放进 about:blank 的 fragment，只做最朴素的转义（没有 HTML 转义库可用）。
fn urlencoding_lite(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' | '<' | '>' | '"' | '&' | '%' | '#' => format!("%{:02X}", c as u32),
            _ => c.to_string(),
        })
        .collect()
}

/// 本机回环用的短口令。不追求密码学强度（攻击面只有本机进程），
/// 但要每次启动都变，避免「上次的 token 还能用来读数据」。
fn local_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in [
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
        std::process::id() as u64,
        (&h as *const u64) as u64,
    ] {
        for byte in v.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{h:016x}")
}
