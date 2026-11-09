use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 凭据不写死在配置里：写 `${ENV:VAR}`，运行时从环境变量取。
pub fn resolve_secret(s: &str) -> Result<String> {
    if let Some(var) = s.strip_prefix("${ENV:").and_then(|x| x.strip_suffix('}')) {
        std::env::var(var).map_err(|_| Error::Config(format!("环境变量 {var} 未设置")))
    } else {
        Ok(s.to_string())
    }
}

fn default_branch() -> String {
    "main".into()
}
fn default_region() -> String {
    "us-east-1".into()
}
fn default_retries() -> u32 {
    3
}
fn default_history() -> usize {
    30
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetConfig {
    /// 本地目录。指向 Syncthing / 坚果云 / OneDrive 的挂载目录即可复用它们做传输。
    Local { id: String, dir: String },
    /// 标准 WebDAV：坚果云、Nextcloud、群晖、ownCloud
    WebDav {
        id: String,
        url: String,
        username: String,
        password: String,
        #[serde(default)]
        root: String,
    },
    /// GitHub Contents API（私有仓库存数据，sha 天然就是 ETag）
    GitHub {
        id: String,
        token: String,
        owner: String,
        repo: String,
        #[serde(default = "default_branch")]
        branch: String,
        #[serde(default)]
        path: String,
    },
    /// 任意 Git remote（GitHub / Gitee / 自建 bare repo），走本地工作树 + push/pull
    Git {
        id: String,
        repo_dir: String,
        #[serde(default)]
        remote: String,
        #[serde(default = "default_branch")]
        branch: String,
    },
    /// S3 兼容对象存储：AWS S3 / Cloudflare R2 / MinIO，AWS SigV4 签名
    S3 {
        id: String,
        endpoint: String,
        bucket: String,
        #[serde(default = "default_region")]
        region: String,
        access_key: String,
        secret_key: String,
        #[serde(default)]
        prefix: String,
    },
}

impl TargetConfig {
    pub fn id(&self) -> &str {
        match self {
            Self::Local { id, .. }
            | Self::WebDav { id, .. }
            | Self::GitHub { id, .. }
            | Self::Git { id, .. }
            | Self::S3 { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub device_id: String,
    pub targets: Vec<TargetConfig>,
    /// 组件级排除，**显式列表**，不做任何启发式推断。
    /// 支持 `events`（不同步复习事件）、`kind:<实体类型>`（上传时排除该类实体）。
    /// 被排除的组件只影响上传，本地数据永远完整。
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    #[serde(default = "default_true")]
    pub upload_snapshots: bool,
    #[serde(default = "default_history")]
    pub keep_snapshots: usize,
}

impl SyncConfig {
    pub fn from_file(p: &Path) -> Result<Self> {
        let bytes = std::fs::read(p).map_err(|e| Error::Config(format!("{}: {e}", p.display())))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn to_file(&self, p: &Path) -> Result<()> {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    pub fn target(&self, id: &str) -> Option<&TargetConfig> {
        self.targets.iter().find(|t| t.id() == id)
    }

    pub fn excludes_events(&self) -> bool {
        self.exclude.iter().any(|s| s == "events")
    }

    pub fn excluded_kinds(&self) -> Vec<String> {
        self.exclude
            .iter()
            .filter_map(|s| s.strip_prefix("kind:").map(String::from))
            .collect()
    }

    /// 生成一份带占位符的示例配置，凭据一律走环境变量。
    pub fn example(device: &str) -> Self {
        Self {
            device_id: device.into(),
            targets: vec![
                TargetConfig::Local {
                    id: "local".into(),
                    dir: "./data/remote".into(),
                },
                TargetConfig::WebDav {
                    id: "jianguoyun".into(),
                    url: "https://dav.jianguoyun.com/dav/".into(),
                    username: "you@example.com".into(),
                    password: "${ENV:MNEME_WEBDAV_TOKEN}".into(),
                    root: "mneme".into(),
                },
                TargetConfig::GitHub {
                    id: "github".into(),
                    token: "${ENV:MNEME_GITHUB_TOKEN}".into(),
                    owner: "your-name".into(),
                    repo: "mneme-data".into(),
                    branch: "main".into(),
                    path: "data".into(),
                },
                TargetConfig::Git {
                    id: "git".into(),
                    repo_dir: "./data/git-mirror".into(),
                    remote: "origin".into(),
                    branch: "main".into(),
                },
                TargetConfig::S3 {
                    id: "r2".into(),
                    endpoint: "https://<accountid>.r2.cloudflarestorage.com".into(),
                    bucket: "mneme".into(),
                    region: "auto".into(),
                    access_key: "${ENV:MNEME_S3_ACCESS_KEY}".into(),
                    secret_key: "${ENV:MNEME_S3_SECRET_KEY}".into(),
                    prefix: "data".into(),
                },
            ],
            exclude: vec![],
            max_retries: 3,
            upload_snapshots: true,
            keep_snapshots: 30,
        }
    }
}
