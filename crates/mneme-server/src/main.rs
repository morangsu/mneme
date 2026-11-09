use clap::Parser;
use mneme_store::Store;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "mneme-server", about = "mneme HTTP 服务（Win 端常驻，安卓浏览器接入）")]
struct Args {
    #[arg(long, default_value = "data")]
    data: PathBuf,
    #[arg(long, default_value_t = 8321)]
    port: u16,
    /// 监听 0.0.0.0（同一 WiFi）。默认只听 127.0.0.1，配合 adb reverse 使用
    #[arg(long)]
    lan: bool,
    /// 开局域网时建议设置访问口令
    #[arg(long)]
    token: Option<String>,
    #[arg(long)]
    device: Option<String>,
    #[arg(long, default_value = "data/sync.json")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let a = Args::parse();
    let device = a
        .device
        .unwrap_or_else(|| std::env::var("COMPUTERNAME").unwrap_or_else(|_| "win".into()));
    let store = Store::open(&a.data, &device)?;
    let engine = mneme_server::load_sync_config(&store, &a.config);
    let state = mneme_server::AppState::new(store, engine, a.token);
    mneme_server::serve(state, a.port, a.lan).await
}
