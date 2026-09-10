// SLS PutLogs authentication.
//
// The Aliyun SLS signature is HMAC-SHA1 over a canonical string:
//
//   VERB \n
//   Content-MD5 \n
//   Content-Type \n
//   Date \n
//   CanonicalizedSLSHeaders \n
//   CanonicalizedResource
//
// where CanonicalizedSLSHeaders is the lowercased, sorted `x-log-*` and
// `x-acs-*` headers joined by `\n` (empty when none). CanonicalizedResource
// is the request path plus a sorted, `&`-joined query string.
//
// See:
//   https://help.aliyun.com/document_detail/29012.html
//
// This mirrors `ossrs::auth::sign` but swaps the OSS header prefix for the
// SLS one and includes any x-acs-* headers (used for the STS security token).

use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::collections::BTreeMap;

type HmacSha1 = Hmac<Sha1>;

/// RFC 1123 timestamp formatted for the `Date` request header.
pub fn time_rfc1123() -> String {
    Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// Canonicalize the SLS-specific request headers: lowercased key, trimmed
/// value, sorted lexicographically, only `x-log-*` and `x-acs-*` prefixes.
fn canonicalize_headers(headers: &BTreeMap<String, String>) -> String {
    headers
        .iter()
        .filter(|(k, _)| {
            let k = k.to_lowercase();
            k.starts_with("x-log-") || k.starts_with("x-acs-")
        })
        .map(|(k, v)| format!("{}:{}", k.to_lowercase(), v.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compute the SLS `Authorization` header value (`LOG <ak>:<signature>`).
///
/// `resource` must be the path *plus* any canonicalized query, e.g.
/// `/logstores/mystore/shards/lb` (no query needed for PutLogs).
#[allow(clippy::too_many_arguments)]
pub fn authorization(
    access_key_id: &str,
    access_key_secret: &str,
    verb: &str,
    content_md5: &str,
    content_type: &str,
    date: &str,
    headers: &BTreeMap<String, String>,
    resource: &str,
) -> String {
    let canonical_headers = canonicalize_headers(headers);
    let mut parts = vec![
        verb.to_string(),
        content_md5.to_string(),
        content_type.to_string(),
        date.to_string(),
    ];
    if !canonical_headers.is_empty() {
        parts.push(canonical_headers);
    }
    parts.push(resource.to_string());
    let sign_string = parts.join("\n");

    let mut mac = HmacSha1::new_from_slice(access_key_secret.as_bytes())
        .expect("HMAC-SHA1 accepts any key size");
    mac.update(sign_string.as_bytes());
    let sig = general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    format!("LOG {}:{}", access_key_id, sig)
}

/// Base64-encoded MD5 of the request body — the value of the `Content-MD5`
/// header that SLS mixes into the signature.
pub fn content_md5(body: &[u8]) -> String {
    use md5::{Digest, Md5};
    let digest = Md5::digest(body);
    // SLS accepts either base64 or uppercase hex; base64 is what the
    // official SDKs use and keeps the header short.
    general_purpose::STANDARD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc1123_has_gmt() {
        let t = time_rfc1123();
        assert!(t.ends_with(" GMT"));
        assert!(t.len() >= 25);
    }

    #[test]
    fn signature_is_stable_for_fixed_inputs() {
        let mut headers = BTreeMap::new();
        headers.insert("x-log-apiversion".to_string(), "0.6.0".to_string());
        headers.insert("x-log-signaturemethod".to_string(), "hmac-sha1".to_string());
        headers.insert("x-log-bodyrawsize".to_string(), "128".to_string());
        headers.insert("x-log-compresstype".to_string(), "lz4".to_string());
        // Non-signed headers must be ignored by the canonicalizer.
        headers.insert("host".to_string(), "proj.cn-hangzhou.log.aliyuncs.com".to_string());
        headers.insert("date".to_string(), "Fri, 19 Jul 2026 12:00:00 GMT".to_string());

        let auth = authorization(
            "test-ak",
            "test-sk",
            "POST",
            "1B2M2Y8AsgTpgAmY7PhCfg==",
            "application/x-protobuf",
            "Fri, 19 Jul 2026 12:00:00 GMT",
            &headers,
            "/logstores/mystore/shards/lb",
        );

        // Stable across runs — recompute inline and compare.
        let expected = {
            let sign_string = concat!(
                "POST\n",
                "1B2M2Y8AsgTpgAmY7PhCfg==\n",
                "application/x-protobuf\n",
                "Fri, 19 Jul 2026 12:00:00 GMT\n",
                "x-log-apiversion:0.6.0\n",
                "x-log-bodyrawsize:128\n",
                "x-log-compresstype:lz4\n",
                "x-log-signaturemethod:hmac-sha1\n",
                "/logstores/mystore/shards/lb",
            );
            let mut mac = HmacSha1::new_from_slice(b"test-sk").unwrap();
            mac.update(sign_string.as_bytes());
            format!(
                "LOG test-ak:{}",
                general_purpose::STANDARD.encode(mac.finalize().into_bytes())
            )
        };
        assert_eq!(auth, expected);
    }

    #[test]
    fn signature_includes_acs_headers() {
        let mut without = BTreeMap::new();
        without.insert("x-log-apiversion".to_string(), "0.6.0".to_string());

        let mut with_sts = without.clone();
        with_sts.insert("x-acs-security-token".to_string(), "sts-token-abc".to_string());

        let a = authorization(
            "ak", "sk", "POST", "", "application/x-protobuf",
            "Fri, 19 Jul 2026 12:00:00 GMT", &without, "/logstores/s/shards/lb",
        );
        let b = authorization(
            "ak", "sk", "POST", "", "application/x-protobuf",
            "Fri, 19 Jul 2026 12:00:00 GMT", &with_sts, "/logstores/s/shards/lb",
        );
        assert_ne!(a, b, "STS token must alter the signature");
    }

    #[test]
    fn content_md5_matches_known_vector() {
        // md5("") base64 = "1B2M2Y8AsgTpgAmY7PhCfg=="
        assert_eq!(content_md5(b""), "1B2M2Y8AsgTpgAmY7PhCfg==");
    }
}
