use crate::model::*;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use mneme_core::content_hash;
use std::{fs, path::PathBuf, sync::Mutex};

/// 公共基础库的读取契约。返回 `None` 表示内容未变（不消耗解析/不产生冲突计划）。
#[async_trait]
pub trait PublicSource: Send + Sync {
    fn id(&self) -> &str;
    async fn fetch_if_changed(&self) -> Result<Option<Vec<u8>>>;
}

/// 本地文件源：离线测试、以及把公共库 clone 到本地后使用。
pub struct FilePublicSource {
    id: String,
    path: PathBuf,
    last: Mutex<Option<String>>,
}

impl FilePublicSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            id: "file".into(),
            path: path.into(),
            last: Mutex::new(None),
        }
    }
}

#[async_trait]
impl PublicSource for FilePublicSource {
    fn id(&self) -> &str {
        &self.id
    }
    async fn fetch_if_changed(&self) -> Result<Option<Vec<u8>>> {
        let bytes = fs::read(&self.path)?;
        let hash = content_hash(&bytes);
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_deref() == Some(hash.as_str()) {
            return Ok(None);
        }
        *last = Some(hash);
        Ok(Some(bytes))
    }
}

/// GitHub 公共库（匿名或用 token）。用 `If-None-Match` 做条件请求，
/// 匿名配额只有 60/h，未变更时返回 304 不消耗解析；命中限流明确报错而不是重试打爆。
pub struct GitHubPublicSource {
    id: String,
    repo: RepoRef,
    token: Option<String>,
    file: String,
    client: reqwest::Client,
    etag: Mutex<Option<String>>,
}

impl GitHubPublicSource {
    pub fn new(repo: RepoRef, file: &str) -> Result<Self> {
        let token = match &repo.token_env {
            Some(var) => Some(
                std::env::var(var)
                    .map_err(|_| Error::Config(format!("环境变量 {var} 未设置")))?,
            ),
            None => None,
        };
        Ok(Self {
            id: repo.slug(),
            repo,
            token,
            file: file.to_string(),
            client: reqwest::Client::new(),
            etag: Mutex::new(None),
        })
    }

    pub fn repo(&self) -> &RepoRef {
        &self.repo
    }

    /// 直接用给定 token。
    ///
    /// Android 上没有环境变量，而匿名请求只有 60 次/小时、且在部分网络下会被直接掐断；
    /// APP 已经为私有库配了凭据，公开库是只读的，复用同一 token 更稳（5000 次/小时）。
    pub fn with_token(repo: RepoRef, file: &str, token: Option<String>) -> Self {
        Self {
            id: repo.slug(),
            repo,
            token,
            file: file.to_string(),
            client: reqwest::Client::new(),
            etag: Mutex::new(None),
        }
    }
}

#[async_trait]
impl PublicSource for GitHubPublicSource {
    fn id(&self) -> &str {
        &self.id
    }
    async fn fetch_if_changed(&self) -> Result<Option<Vec<u8>>> {
        let url = format!(
            "{}?ref={}",
            self.repo.contents_url(&self.file),
            self.repo.branch
        );
        let etag = self
            .etag
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut req = self
            .client
            .get(&url)
            .header("User-Agent", "mneme-upstream")
            .header("Accept", "application/vnd.github+json");
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        if let Some(e) = &etag {
            req = req.header("If-None-Match", e);
        }
        let resp = req.send().await?;
        let code = resp.status().as_u16();
        if code == 304 {
            return Ok(None);
        }
        if code == 403 || code == 429 {
            let reset = resp
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("未知")
                .to_string();
            return Err(Error::RateLimited(reset));
        }
        if code == 404 {
            return Err(Error::Bundle(format!("公共库里没有 {}", self.file)));
        }
        if !resp.status().is_success() {
            return Err(Error::Bundle(format!("GitHub 返回 {code}")));
        }
        let body: serde_json::Value = resp.json().await?;
        let sha = body["sha"].as_str().unwrap_or_default().to_string();
        let raw = body["content"].as_str().unwrap_or_default();
        let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = STANDARD
            .decode(compact.as_bytes())
            .map_err(|e| Error::Bundle(format!("base64: {e}")))?;
        if !sha.is_empty() {
            *self.etag.lock().unwrap_or_else(|e| e.into_inner()) = Some(sha);
        }
        Ok(Some(bytes))
    }
}
