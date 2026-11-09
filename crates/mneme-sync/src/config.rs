use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 凭据不写死在配置里，两种引用方式：
/// - `${ENV:VAR}`：从环境变量取（桌面端 / CI）。
/// - `${SECRET:name}`：从宿主注入的**进程内注册表**取（Android 走 Keystore 解密后注入）。
///
/// 磁盘上的 `sync.json` 只保存引用，不保存真值。
pub fn resolve_secret(s: &str) -> Result<String> {
    if let Some(var) = s.strip_prefix("${ENV:").and_then(|x| x.strip_suffix('}')) {
        return std::env::var(var)
            .map_err(|_| Error::Config(format!("环境变量 {var} 未设置")));
    }
    if let Some(name) = s.strip_prefix("${SECRET:").and_then(|x| x.strip_suffix('}')) {
        return crate::secret::get(name).ok_or_else(|| {
            Error::Config(format!(
                "凭据 {name} 未注入：宿主需要在启动时 setCredential(\"{name}\", …)"
            ))
        });
    }
    Ok(s.to_string())
}

/// Debug 里显示凭据占位：真值一律 `***`，只区分「有」和「没有」。
///
/// 这不是洁癖：`{:?}` 会出现在 panic message、调试日志、错误链里，
/// 而一个 token 泄露就够别人读写整个私有库。占位符 `${SECRET:x}` 也一起打码 ——
/// 少一个「这个字段到底是不是引用」的判断，就少一类漏判。
fn mask(s: &str) -> &'static str {
    if s.is_empty() { "<empty>" } else { "***" }
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

#[derive(Clone, Serialize, Deserialize, PartialEq)]
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

impl std::fmt::Debug for TargetConfig {
    /// 手写实现而非 derive：`token`/`password`/`secret_key` 必须打码，
    /// 否则任何 `{:?}`（panic、日志、错误回执）都可能把凭据带出去。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local { id, dir } => f
                .debug_struct("Local")
                .field("id", id)
                .field("dir", dir)
                .finish(),
            Self::WebDav {
                id,
                url,
                username,
                password,
                root,
            } => f
                .debug_struct("WebDav")
                .field("id", id)
                .field("url", url)
                .field("username", username)
                .field("password", &mask(password))
                .field("root", root)
                .finish(),
            Self::GitHub {
                id,
                token,
                owner,
                repo,
                branch,
                path,
            } => f
                .debug_struct("GitHub")
                .field("id", id)
                .field("token", &mask(token))
                .field("owner", owner)
                .field("repo", repo)
                .field("branch", branch)
                .field("path", path)
                .finish(),
            Self::Git {
                id,
                repo_dir,
                remote,
                branch,
            } => f
                .debug_struct("Git")
                .field("id", id)
                .field("repo_dir", repo_dir)
                .field("remote", remote)
                .field("branch", branch)
                .finish(),
            Self::S3 {
                id,
                endpoint,
                bucket,
                region,
                access_key,
                secret_key,
                prefix,
            } => f
                .debug_struct("S3")
                .field("id", id)
                .field("endpoint", endpoint)
                .field("bucket", bucket)
                .field("region", region)
                .field("access_key", &mask(access_key))
                .field("secret_key", &mask(secret_key))
                .field("prefix", prefix)
                .finish(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn gh(token: &str) -> SyncConfig {
        SyncConfig {
            device_id: "dev".into(),
            targets: vec![TargetConfig::GitHub {
                id: "github-private".into(),
                token: token.into(),
                owner: "o".into(),
                repo: "r".into(),
                branch: "main".into(),
                path: "data".into(),
            }],
            exclude: vec![],
            max_retries: 3,
            upload_snapshots: true,
            keep_snapshots: 30,
        }
    }

    #[test]
    fn debug_output_never_contains_the_secret() {
        // 这条断言的意义：任何 `{:?}` 都不会把 token 带进日志 / panic / 错误回执
        let cfg = gh("ghp_LEAKME1234567890");
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("ghp_LEAKME"), "Debug 泄露了 token: {dbg}");
        assert!(dbg.contains("github-private"));
        // 空凭据也要有区分度，否则排查「为什么 401」时看不出配没配
        assert!(format!("{:?}", gh("")).contains("<empty>"));
    }

    #[test]
    fn secret_reference_resolves_from_registry() {
        let _g = crate::secret::serialize_tests();
        crate::secret::clear();
        assert!(crate::secret::put("github-private", "ghp_from_vault"));
        assert_eq!(
            resolve_secret("${SECRET:github-private}").unwrap(),
            "ghp_from_vault"
        );
        // 没注入时必须报错，而不是拿占位符当 token 去请求（那样会 401，误导排查）
        let e = resolve_secret("${SECRET:missing}").unwrap_err();
        assert!(e.to_string().contains("未注入"));
        crate::secret::clear();
    }

    #[test]
    fn plain_secrets_and_missing_env_are_handled() {
        // 明文仍旧允许（桌面端单机、或用户确实想写在配置里）
        assert_eq!(resolve_secret("plain-token").unwrap(), "plain-token");
        // 环境变量缺失要报错，不能静默当成空 token 去请求
        assert!(resolve_secret("${ENV:MNEME_TEST_VAR_NOPE}").is_err());
    }
}
