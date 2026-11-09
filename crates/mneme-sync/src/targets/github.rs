use crate::{
    error::{Error, Result},
    traits::{RemoteMeta, SyncTarget, safe_rel},
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use mneme_core::content_hash;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct GhContent {
    #[serde(default)]
    sha: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(rename = "type", default)]
    typ: Option<String>,
}

/// GitHub Contents API。文件 `sha` 天然就是 ETag：
/// PUT 时带上它，远端内容变了就返回 409/422 -> 触发引擎的 412 自愈。
pub struct GitHub {
    id: String,
    token: String,
    owner: String,
    repo: String,
    branch: String,
    path: String,
    client: reqwest::Client,
}

impl GitHub {
    pub fn new(
        id: impl Into<String>,
        token: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
        branch: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            token: token.into(),
            owner: owner.into(),
            repo: repo.into(),
            branch: branch.into(),
            path: path.into().trim_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    fn api(&self, p: &str) -> Result<String> {
        let rel = safe_rel(p)?;
        let prefix = if self.path.is_empty() {
            String::new()
        } else {
            format!("{}/", safe_rel(&self.path)?)
        };
        Ok(format!(
            "https://api.github.com/repos/{}/{}/contents/{}{}",
            self.owner, self.repo, prefix, rel
        ))
    }

    fn auth(&self, b: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        b.header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(USER_AGENT, "mneme-sync")
    }

    async fn fetch(&self, p: &str) -> Result<Option<GhContent>> {
        let url = format!("{}?ref={}", self.api(p)?, self.branch);
        let resp = self.auth(self.client.get(&url)).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(Error::Remote {
                status: resp.status().as_u16(),
                body: format!("GET {p}"),
            });
        }
        Ok(Some(resp.json::<GhContent>().await?))
    }

    async fn list_recursive(&self, p: &str, depth: u8, out: &mut Vec<String>) -> Result<()> {
        if depth > 4 {
            return Ok(());
        }
        let url = format!("{}?ref={}", self.api(p)?, self.branch);
        let resp = self.auth(self.client.get(&url)).send().await?;
        if !resp.status().is_success() {
            return Ok(());
        }
        let items: Vec<GhContent> = resp.json().await.unwrap_or_default();
        for it in items {
            let name = it.path.unwrap_or_else(|| it.name.unwrap_or_default());
            if it.typ.as_deref() == Some("dir") {
                Box::pin(self.list_recursive(&name, depth + 1, out)).await?;
            } else if !name.is_empty() {
                out.push(name);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl SyncTarget for GitHub {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "github"
    }

    async fn head(&self, path: &str) -> Result<Option<RemoteMeta>> {
        Ok(self.fetch(path).await?.map(|c| RemoteMeta {
            etag: c.sha,
            size: c.size.unwrap_or(0),
        }))
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>> {
        let c = self.fetch(path).await?.ok_or_else(|| Error::NotFound(path.into()))?;
        let raw = c.content.ok_or_else(|| Error::Sync(format!("{path} 没有 content 字段")))?;
        let compact: String = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
        STANDARD
            .decode(compact.as_bytes())
            .map_err(|e| Error::Sync(format!("base64 解码失败: {e}")))
    }

    async fn put(&self, path: &str, data: &[u8], if_match: Option<&str>) -> Result<RemoteMeta> {
        let url = self.api(path)?;
        let mut body = serde_json::json!({
            "message": format!("mneme sync {}", path),
            "content": STANDARD.encode(data),
            "branch": self.branch,
        });
        if let Some(sha) = if_match {
            body["sha"] = serde_json::Value::String(sha.to_string());
        }
        let resp = self.auth(self.client.put(&url)).json(&body).send().await?;
        let code = resp.status().as_u16();
        if code == 409 || code == 422 || code == 412 {
            return Err(Error::PreconditionFailed(format!("GitHub PUT {path} -> {code}")));
        }
        if !resp.status().is_success() {
            return Err(Error::Remote {
                status: code,
                body: format!("PUT {path}: {}", resp.text().await.unwrap_or_default()),
            });
        }
        let sha = resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["content"]["sha"].as_str().map(String::from))
            .unwrap_or_else(|| content_hash(data));
        Ok(RemoteMeta {
            etag: sha,
            size: data.len() as u64,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let p = safe_rel(prefix)?;
        let mut out = Vec::new();
        self.list_recursive(if p.is_empty() { "." } else { &p }, 0, &mut out)
            .await?;
        // 返回值统一成「相对 path 前缀」的形式
        let base = if self.path.is_empty() {
            String::new()
        } else {
            format!("{}/", self.path)
        };
        out = out
            .into_iter()
            .map(|s| s.strip_prefix(&base).unwrap_or(&s).to_string())
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let sha = match self.head(path).await? {
            Some(m) => m.etag,
            None => return Ok(()),
        };
        let url = self.api(path)?;
        let body = serde_json::json!({
            "message": format!("mneme delete {}", path),
            "sha": sha,
            "branch": self.branch,
        });
        let resp = self.auth(self.client.delete(&url)).json(&body).send().await?;
        if !resp.status().is_success() && resp.status().as_u16() != 404 {
            return Err(Error::Remote {
                status: resp.status().as_u16(),
                body: format!("DELETE {path}"),
            });
        }
        Ok(())
    }
}
