use crate::{
    error::{Error, Result},
    traits::{RemoteMeta, SyncTarget, safe_rel},
};
use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use mneme_core::content_hash;
use reqwest::header::IF_MATCH;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type HmacSha256 = Hmac<Sha256>;

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC 接受任意长度 key");
    m.update(msg);
    m.finalize().into_bytes().to_vec()
}

fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// AWS Signature V4 的签名密钥派生链。公开以便用固定向量做单测。
pub fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// S3 兼容对象存储（AWS S3 / Cloudflare R2 / MinIO），path-style 寻址。
/// 只依赖 sha2/hmac/chrono，不引整套 AWS SDK。
pub struct S3Target {
    id: String,
    endpoint: String,
    bucket: String,
    region: String,
    ak: String,
    sk: String,
    prefix: String,
    client: reqwest::Client,
}

struct Signed {
    url: String,
    headers: BTreeMap<String, String>,
}

impl S3Target {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        region: impl Into<String>,
        ak: impl Into<String>,
        sk: impl Into<String>,
        prefix: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            bucket: bucket.into(),
            region: region.into(),
            ak: ak.into(),
            sk: sk.into(),
            prefix: prefix.into().trim_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    fn uri(&self, p: &str) -> Result<String> {
        let rel = safe_rel(p)?;
        let mut segs = vec![self.bucket.clone()];
        if !self.prefix.is_empty() {
            segs.push(safe_rel(&self.prefix)?);
        }
        if !rel.is_empty() {
            segs.push(rel);
        }
        Ok(format!("/{}", segs.join("/")))
    }

    fn sign(
        &self,
        method: &str,
        uri: &str,
        query: &BTreeMap<String, String>,
        payload: &[u8],
        if_match: Option<&str>,
    ) -> Result<Signed> {
        let now = Utc::now();
        let amzdate = now.format("%Y%m%dT%H%M%SZ").to_string();
        let datestamp = now.format("%Y%m%d").to_string();
        let host = self
            .endpoint
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        let payload_hash = sha256_hex(payload);
        let canonical_query = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        headers.insert("host".into(), host);
        headers.insert("x-amz-content-sha256".into(), payload_hash.clone());
        headers.insert("x-amz-date".into(), amzdate.clone());
        if let Some(et) = if_match {
            headers.insert("if-match".into(), et.to_string());
        }
        let canonical_headers = headers
            .iter()
            .map(|(k, v)| format!("{}:{}\n", k, v.trim()))
            .collect::<String>();
        let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");

        let canonical_request = format!(
            "{method}\n{uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        let scope = format!("{}/{}/s3/aws4_request", datestamp, self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amzdate}\n{scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );
        let key = signing_key(&self.sk, &datestamp, &self.region, "s3");
        let signature = hex::encode(hmac_sha256(&key, string_to_sign.as_bytes()));
        headers.insert(
            "authorization".into(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                self.ak
            ),
        );
        let url = if canonical_query.is_empty() {
            format!("{}{}", self.endpoint, uri)
        } else {
            format!("{}{}?{}", self.endpoint, uri, canonical_query)
        };
        Ok(Signed { url, headers })
    }

    fn apply(req: reqwest::RequestBuilder, s: &Signed) -> reqwest::RequestBuilder {
        let mut r = req;
        for (k, v) in &s.headers {
            r = r.header(k.as_str(), v.as_str());
        }
        r
    }

    fn parse_keys(xml: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = xml;
        while let Some(i) = rest.find("<Key>") {
            let start = i + 5;
            let tail = &rest[start..];
            if let Some(end) = tail.find("</Key>") {
                out.push(tail[..end].to_string());
                rest = &tail[end + 6..];
            } else {
                break;
            }
        }
        out
    }
}

#[async_trait]
impl SyncTarget for S3Target {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "s3"
    }

    async fn head(&self, path: &str) -> Result<Option<RemoteMeta>> {
        let uri = self.uri(path)?;
        let s = self.sign("HEAD", &uri, &BTreeMap::new(), b"", None)?;
        let resp = S3Target::apply(self.client.head(&s.url), &s).send().await?;
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
            .map(|x| x.trim_matches('"').to_string())
            .unwrap_or_else(|| content_hash(path.as_bytes()));
        let size = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        Ok(Some(RemoteMeta { etag, size }))
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>> {
        let uri = self.uri(path)?;
        let s = self.sign("GET", &uri, &BTreeMap::new(), b"", None)?;
        let resp = S3Target::apply(self.client.get(&s.url), &s).send().await?;
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
        let uri = self.uri(path)?;
        let s = self.sign("PUT", &uri, &BTreeMap::new(), data, if_match)?;
        let mut req = S3Target::apply(self.client.put(&s.url), &s).body(data.to_vec());
        if let Some(et) = if_match {
            req = req.header(IF_MATCH, et);
        }
        let resp = req.send().await?;
        let code = resp.status().as_u16();
        if code == 412 {
            return Err(Error::PreconditionFailed(format!("S3 PUT {path} -> 412")));
        }
        if !resp.status().is_success() {
            return Err(Error::Remote {
                status: code,
                body: format!("PUT {path}: {}", resp.text().await.unwrap_or_default()),
            });
        }
        Ok(RemoteMeta {
            etag: resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|x| x.trim_matches('"').to_string())
                .unwrap_or_else(|| content_hash(data)),
            size: data.len() as u64,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let uri = self.uri("")?;
        let mut q = BTreeMap::new();
        q.insert("list-type".into(), "2".into());
        let mut full_prefix = String::new();
        if !self.prefix.is_empty() {
            full_prefix.push_str(&self.prefix);
            full_prefix.push('/');
        }
        full_prefix.push_str(&safe_rel(prefix)?);
        q.insert("prefix".into(), full_prefix.clone());
        let s = self.sign("GET", &uri, &q, b"", None)?;
        let resp = S3Target::apply(self.client.get(&s.url), &s).send().await?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }
        let xml = resp.text().await?;
        let base = format!("{}/", self.prefix.trim_matches('/'));
        let mut out: Vec<String> = S3Target::parse_keys(&xml)
            .into_iter()
            .map(|k| k.strip_prefix(&base).unwrap_or(&k).to_string())
            .filter(|k| !k.is_empty())
            .collect();
        out.sort();
        Ok(out)
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let uri = self.uri(path)?;
        let s = self.sign("DELETE", &uri, &BTreeMap::new(), b"", None)?;
        let resp = S3Target::apply(self.client.delete(&s.url), &s).send().await?;
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

    /// RFC 4231 test case 2，验证 HMAC-SHA256 基础件没写错
    #[test]
    fn hmac_rfc4231_vector() {
        let got = hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?"));
        assert_eq!(
            got,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn signing_key_is_deterministic_32_bytes() {
        let a = signing_key("secret", "20261109", "us-east-1", "s3");
        let b = signing_key("secret", "20261109", "us-east-1", "s3");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        assert_ne!(a, signing_key("secret", "20261110", "us-east-1", "s3"));
        assert_ne!(a, signing_key("secret", "20261109", "eu-west-1", "s3"));
    }

    #[test]
    fn signature_contains_expected_scope_and_headers() {
        let t = S3Target::new(
            "t",
            "https://example.com",
            "bucket",
            "us-east-1",
            "AKID",
            "SECRET",
            "data",
        );
        let s = t
            .sign("PUT", "/bucket/data/latest.json", &BTreeMap::new(), b"{}", None)
            .unwrap();
        let auth = &s.headers["authorization"];
        assert!(auth.contains("Credential=AKID/"));
        assert!(auth.contains("/us-east-1/s3/aws4_request"));
        assert!(auth.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        assert!(s.url.ends_with("/bucket/data/latest.json"));
    }

    #[test]
    fn parses_list_objects_xml() {
        let xml = "<ListBucketResult><Contents><Key>data/events/win.jsonl</Key></Contents><Contents><Key>data/latest.json</Key></Contents></ListBucketResult>";
        assert_eq!(S3Target::parse_keys(xml).len(), 2);
    }
}
