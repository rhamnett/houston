//! AWS Signature Version 4, for the few JSON calls Houston makes. Built on
//! `ring` (already here for TLS), so there's no AWS SDK and no `aws` CLI.
//! <https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html>

use chrono::{DateTime, Utc};
use ring::{digest, hmac};

/// What a request is signed with.
#[derive(Clone)]
pub struct Keys {
    pub id: String,
    pub secret: String,
    /// Temporary credentials (SSO, a role, a credential process) come with one.
    pub token: Option<String>,
}

/// Never print the secret or the token.
impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys")
            .field("id", &self.id)
            .field("token", &self.token.as_ref().map(|_| "…"))
            .finish()
    }
}

pub struct Request<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    /// Already canonical: names and values encoded, pairs sorted.
    pub query: &'a str,
    /// Headers to sign besides `host` and the `x-amz-*` ones added here.
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(data: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, data).as_ref())
}

fn mac(key: &[u8], data: &str) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data.as_bytes())
        .as_ref()
        .to_vec()
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k = mac(format!("AWS4{secret}").as_bytes(), date);
    let k = mac(&k, region);
    let k = mac(&k, service);
    mac(&k, "aws4_request")
}

/// The headers to send on top of `req.headers`: `x-amz-date`, the session
/// token if there is one, and `authorization`.
pub fn sign(
    req: &Request,
    keys: &Keys,
    region: &str,
    service: &str,
    now: DateTime<Utc>,
) -> Vec<(String, String)> {
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = &stamp[..8];
    let mut extra = vec![("x-amz-date".to_string(), stamp.clone())];
    if let Some(token) = &keys.token {
        extra.push(("x-amz-security-token".into(), token.clone()));
    }
    let mut signed: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
        .chain([("host".to_string(), req.host.to_string())])
        .chain(extra.iter().cloned())
        .collect();
    signed.sort();
    let names = signed
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers: String = signed.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let canonical = format!(
        "{}\n{}\n{}\n{canonical_headers}\n{names}\n{}",
        req.method,
        req.path,
        req.query,
        sha256(req.body)
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        sha256(canonical.as_bytes())
    );
    let signature = hex(&mac(
        &signing_key(&keys.secret, date, region, service),
        &to_sign,
    ));
    extra.push((
        "authorization".into(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={names}, Signature={signature}",
            keys.id
        ),
    ));
    extra
}

#[cfg(test)]
mod tests {
    use super::*;

    // From the AWS Signature Version 4 test suite.
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    fn keys(token: Option<&str>) -> Keys {
        Keys {
            id: "AKIDEXAMPLE".into(),
            secret: SECRET.into(),
            token: token.map(String::from),
        }
    }

    fn at_1236() -> DateTime<Utc> {
        "2015-08-30T12:36:00Z".parse().unwrap()
    }

    fn authorization(headers: &[(String, String)]) -> &str {
        &headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap()
            .1
    }

    #[test]
    fn signing_key_matches_the_docs() {
        let k = signing_key(SECRET, "20120215", "us-east-1", "iam");
        assert_eq!(
            hex(&k),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    fn vanilla(method: &str) -> Vec<(String, String)> {
        let req = Request {
            method,
            host: "example.amazonaws.com",
            path: "/",
            query: "",
            headers: &[],
            body: b"",
        };
        sign(&req, &keys(None), "us-east-1", "service", at_1236())
    }

    #[test]
    fn get_vanilla() {
        let h = vanilla("GET");
        assert_eq!(h[0], ("x-amz-date".into(), "20150830T123600Z".into()));
        assert_eq!(
            authorization(&h),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn post_vanilla() {
        assert!(authorization(&vanilla("POST")).ends_with(
            "Signature=5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
        ));
    }

    #[test]
    fn a_session_token_is_signed_and_sent() {
        let req = Request {
            method: "POST",
            host: "example.amazonaws.com",
            path: "/",
            query: "",
            headers: &[("Content-Type", "application/x-amz-json-1.0")],
            body: b"{}",
        };
        let h = sign(&req, &keys(Some("tok")), "us-east-1", "service", at_1236());
        assert!(h.contains(&("x-amz-security-token".into(), "tok".into())));
        assert!(
            authorization(&h)
                .contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token,")
        );
        assert!(!format!("{:?}", keys(Some("tok"))).contains(SECRET));
    }
}
