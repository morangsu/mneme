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
    let device = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "desktop".into());

    // 1) 打开库（卡片懒加载：矩阵页不需要卡片）。
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

    // 2) 同进程起 axum：随机端口 + 随机 token，只听本机回环。
    //    随机 token 的作用：别的本机进程就算知道端口，也拿不到数据。
    let port = args.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    let token = local_token();
    let state = AppState::new(store, None, Some(token.clone()));
    let server = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let (port_tx, port_rx) = std::sync::mpsc::channel::<u16>();
    std::thread::spawn(move || {
        server.block_on(async move {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .expect("绑定 127.0.0.1 失败");
            let port = listener.local_addr().expect("local_addr").port();
            let _ = port_tx.send(port);
            if let Err(e) = mneme_server::serve_on(state, listener).await {
                eprintln!("server 退出: {e}");
            }
        });
    });
    let port = port_rx.recv().context("server 启动失败")?;
    let url = format!("http://127.0.0.1:{port}/?token={token}");
    println!(
        "[t= {:>7.1} ms] 服务已监听 {url}",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // 3) 开窗 + WebView。
    use tao::event_loop::EventLoop;
    use tao::window::WindowBuilder;
    use wry::{PageLoadEvent, WebViewBuilder};

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("mneme")
        .with_inner_size(tao::dpi::LogicalSize::new(1180.0, 780.0))
        .build(&event_loop)?;
    let webview = WebViewBuilder::new()
        .with_url(&url)
        .with_on_page_load_handler(move |event, _| {
            // 只关心「加载完成」：这是用户真正看到内容的时刻
            if matches!(event, PageLoadEvent::Finished) {
                println!(
                    "[t= {:>7.1} ms] 页面加载完成（首帧）",
                    t0.elapsed().as_secs_f64() * 1000.0
                );
            }
        })
        .build(&window)
        .context("创建 WebView 失败（需要 WebView2 Runtime）")?;
    println!(
        "[t= {:>7.1} ms] WebView 已创建",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    event_loop.run(move |event, _, control_flow| {
        if let tao::event::Event::WindowEvent {
            event: tao::event::WindowEvent::CloseRequested,
            ..
        } = event
        {
            // 关窗即退出：数据都在事件日志里，没有需要优雅关闭的东西
            *control_flow = tao::event_loop::ControlFlow::Exit;
        }
        let _ = &webview; // 保持 WebView 存活
    });
    Ok(())
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
