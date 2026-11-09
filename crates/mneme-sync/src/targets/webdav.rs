use crate::{
    error::{Error, Result},
    traits::{RemoteMeta, SyncTarget, safe_rel},
};
use async_trait::async_trait;
use mneme_core::content_hash;
use reqwest::header::IF_MATCH;

/// 标准 WebDAV：坚果云 / Nextcloud / 群晖 / ownCloud。
/// 用 `If-Match` 做乐观并发，412 交给引擎自愈。
pub struct WebDav {
    id: String,
    base: String,
    user: String,
    pass: String,
    root: String,
    client: reqwest::Client,
}

impl WebDav {
    pub fn new(
        id: impl Into<String>,
        url: impl Into<String>,
        user: impl Into<String>,
        pass: impl Into<String>,
        root: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            base: url.into().trim_end_matches('/').to_string(),
            user: user.into(),
            pass: pass.into(),
            root: root.into().trim_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    fn url(&self, p: &str) -> Result<String> {
        let rel = safe_rel(p)?;
        let mut parts: Vec<String> = Vec::new();
        if !self.root.is_empty() {
            parts.push(safe_rel(&self.root)?);
        }
        parts.push(rel);
        Ok(format!("{}/{}", self.base, parts.join("/")))
    }

    async fn status(&self, method: &str, url: &str) -> Result<reqwest::Response> {
        let builder = match method {
            "HEAD" => self.client.head(url),
            "GET" => self.client.get(url),
            _ => return Err(Error::Sync(format!("不支持的 method {method}"))),
        };
        builder
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .await
            .map_err(Into::into)
    }

    /// 逐级 MKCOL，405 表示目录已存在。
    async fn ensure_dirs(&self, p: &str) -> Result<()> {
        let rel = safe_rel(p)?;
        let segs: Vec<&str> = rel.split('/').collect();
        let mut acc: Vec<String> = Vec::new();
        if !self.root.is_empty() {
            acc.push(safe_rel(&self.root)?);
        }
        for seg in segs.iter().take(segs.len().saturating_sub(1)) {
            acc.push((*seg).to_string());
            let url = format!("{}/{}", self.base, acc.join("/"));
            let _ = self
                .client
                .request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), &url)
                .basic_auth(&self.user, Some(&self.pass))
                .send()
                .await;
        }
        Ok(())
    }

    fn parse_hrefs(xml: &str) -> Vec<String> {
        let lower = xml.to_ascii_lowercase();
        let mut out = Vec::new();
        let mut pos = 0usize;
        while let Some(i) = lower[pos..].find("href") {
            let abs = pos + i;
            if let Some(gt) = xml[abs..].find('>') {
                let start = abs + gt + 1;
                if let Some(lt) = xml[start..].find('<') {
                    let s = xml[start..start + lt].trim();
                    if !s.is_empty() && !s.starts_with("http") {
                        out.push(s.to_string());
                    } else if let Some(u) = s.strip_prefix("http") {
                        // 绝对 href：取路径部分
                        if let Some(idx) = u.find('/') {
                            if let Some(slash) = u[idx..].find('/') {
                                out.push(u[idx + slash + 1..].to_string());
                            }
                        }
                    }
                }
            }
            pos = (abs + 4).min(lower.len());
            if pos >= lower.len() {
                break;
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

#[async_trait]
impl SyncTarget for WebDav {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "webdav"
    }

    async fn head(&self, path: &str) -> Result<Option<RemoteMeta>> {
        let url = self.url(path)?;
        let resp = self.status("HEAD", &url).await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(Error::Remote {
                status: resp.status().as_u16(),
                body: format!("HEAD {path}"),
            });
        }
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_matches('"').to_string());
        let size = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        // 有些服务不给 ETag，退回 GET 内容哈希，保证 412 判定准确
        let etag = match etag {
            Some(e) if !e.is_empty() => e,
            _ => content_hash(&self.get(path).await?),
        };
        Ok(Some(RemoteMeta { etag, size }))
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>> {
        let url = self.url(path)?;
        let resp = self.status("GET", &url).await?;
        let code = resp.status().as_u16();
        if code == 404 {
            return Err(Error::NotFound(path.into()));
        }
        if !resp.status().is_success() {
            return Err(Error::Remote {
                status: code,
                body: format!("GET {path}"),
            });
        }
        Ok(resp.bytes().await?.to_vec())
    }

    async fn put(&self, path: &str, data: &[u8], if_match: Option<&str>) -> Result<RemoteMeta> {
        let url = self.url(path)?;
        self.ensure_dirs(path).await?;
        let mut req = self
            .client
            .put(&url)
            .basic_auth(&self.user, Some(&self.pass))
            .body(data.to_vec());
        if let Some(et) = if_match {
            req = req.header(IF_MATCH, et);
        }
        let resp = req.send().await?;
        let code = resp.status().as_u16();
        if code == 412 || code == 428 {
            return Err(Error::PreconditionFailed(format!("WebDAV PUT {path} -> {code}")));
        }
        if !resp.status().is_success() {
            return Err(Error::Remote {
                status: code,
                body: format!("PUT {path}: {}", resp.text().await.unwrap_or_default()),
            });
        }
        Ok(RemoteMeta {
            etag: content_hash(data),
            size: data.len() as u64,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let url = self.url(if prefix.is_empty() { "." } else { prefix })?;
        let resp = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .basic_auth(&self.user, Some(&self.pass))
            .header("Depth", "infinity")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }
        let xml = resp.text().await?;
        let mut out = WebDav::parse_hrefs(&xml);
        let clean = safe_rel(prefix)?;
        out.retain(|s| s.starts_with(&clean));
        Ok(out)
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let url = self.url(path)?;
        let resp = self
            .client
            .delete(&url)
            .basic_auth(&self.user, Some(&self.pass))
            .send()
            .await?;
        if !resp.status().is_success() && resp.status().as_u16() != 404 {
            return Err(Error::Remote {
                status: resp.status().as_u16(),
                body: format!("DELETE {path}"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_webdav_multistatus() {
        let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
<d:response><d:href>/dav/mneme/events/win.jsonl</d:href></d:response>
<d:response><d:href>/dav/mneme/snapshots/snap-00000001-abcd.json</d:href></d:response>
</d:multistatus>"#;
        let hrefs = WebDav::parse_hrefs(xml);
        assert!(hrefs.iter().any(|s| s.contains("win.jsonl")));
        assert!(hrefs.iter().any(|s| s.contains("snap-")));
    }
}
