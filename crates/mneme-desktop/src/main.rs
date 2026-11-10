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
            let token = local_token();
            let state = AppState::new(store, None, Some(token.clone()));
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
                *control_flow = if pending.is_some() {
                    ControlFlow::Poll
                } else {
                    ControlFlow::Wait
                };
            }
            tao::event::Event::WindowEvent {
                event: tao::event::WindowEvent::CloseRequested,
                ..
            } => {
                // 关窗即退出：数据都在事件日志里，没有需要优雅关闭的东西
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
        let _ = &webview; // 保持 WebView 存活
    });
    Ok(())
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
