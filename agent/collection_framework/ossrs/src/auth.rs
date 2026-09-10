use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::collections::HashMap;

type HmacSha1 = Hmac<Sha1>;

/// Generate RFC 1123 formatted date string
pub fn time_rfc1123() -> String {
    Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// Extract and format OSS headers for signing
fn get_oss_headers_for_signing(oss_headers: &HashMap<String, String>) -> Vec<String> {
    let mut sorted_keys: Vec<_> = oss_headers
        .keys()
        .filter(|k| k.to_lowercase().starts_with("x-oss-"))
        .collect();
    sorted_keys.sort();

    sorted_keys
        .iter()
        .map(|k| format!("{}:{}", k.to_lowercase(), oss_headers[*k]))
        .collect()
}

/// Generate HMAC-SHA1 signature
fn sign(
    sk: &str,
    uri: &str,
    content_type: &str,
    date: &str,
    verb: &str,
    oss_headers: &HashMap<String, String>,
) -> String {
    let mut string_to_sign = vec![
        verb.to_string(),
        String::new(), // Content-MD5 (empty)
        content_type.to_string(),
        date.to_string(),
    ];

    // Add OSS headers
    string_to_sign.extend(get_oss_headers_for_signing(oss_headers));
    string_to_sign.push(uri.to_string());

    let sign_string = string_to_sign.join("\n");

    let mut mac = HmacSha1::new_from_slice(sk.as_bytes()).expect("HMAC can take key of any size");
    mac.update(sign_string.as_bytes());
    let result = mac.finalize();

    general_purpose::STANDARD.encode(result.into_bytes())
}

/// Generate authorization header
pub fn auth(
    ak: &str,
    sk: &str,
    uri: &str,
    content_type: &str,
    date: &str,
    verb: &str,
    oss_headers: &HashMap<String, String>,
) -> String {
    let signature = sign(sk, uri, content_type, date, verb, oss_headers);
    format!("OSS {}:{}", ak, signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_rfc1123() {
        let time = time_rfc1123();
        assert!(!time.is_empty());
        assert!(time.contains("GMT"));
    }

    #[test]
    fn test_auth() {
        let mut oss_headers = HashMap::new();
        oss_headers.insert("x-oss-object-acl".to_string(), "private".to_string());

        let auth_str = auth(
            "test_ak",
            "test_sk",
            "/bucket/object",
            "application/octet-stream",
            "Wed, 20 Jan 2026 00:00:00 GMT",
            "PUT",
            &oss_headers,
        );

        assert!(auth_str.starts_with("OSS test_ak:"));
    }
}
