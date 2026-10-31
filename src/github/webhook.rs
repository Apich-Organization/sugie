use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn verify_signature(secret: &str, signature_header: Option<&str>, body: &[u8]) -> bool {
    // HMAC accepts a zero-length key, so an unset secret would still produce a
    // tag that verifies — and everyone can compute `HMAC(key="", body)`. Treat
    // "not configured" as "reject everything" rather than as "no check".
    if secret.is_empty() {
        return false;
    }

    let sig_header = match signature_header {
        Some(s) => s,
        None => return false,
    };

    let hex_sig = if let Some(stripped) = sig_header.strip_prefix("sha256=") {
        stripped
    } else {
        return false;
    };

    let expected_sig = match hex::decode(hex_sig) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    mac.update(body);
    mac.verify_slice(&expected_sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_signature() {
        let secret = "test-secret";
        let payload = b"{\"action\":\"opened\"}";

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload);
        let result = mac.finalize();
        let sig_header = format!("sha256={}", hex::encode(result.into_bytes()));

        assert!(verify_signature(secret, Some(&sig_header), payload));
    }

    #[test]
    fn test_invalid_signature() {
        let secret = "test-secret";
        let payload = b"{\"action\":\"opened\"}";
        let bad_sig = "sha256=1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

        assert!(!verify_signature(secret, Some(bad_sig), payload));
        assert!(!verify_signature(secret, None, payload));
        assert!(!verify_signature("wrong-secret", Some(bad_sig), payload));
    }

    #[test]
    fn test_empty_secret_rejects_everything() {
        // Regression for A1: an unconfigured secret must not degrade into "no
        // verification". `HMAC(key="", body)` is computable by anyone, so a
        // forged header built that way has to be rejected.
        let payload = b"{\"action\":\"opened\"}";

        let mut mac = HmacSha256::new_from_slice(b"").unwrap();
        mac.update(payload);
        let forged = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));

        assert!(!verify_signature("", Some(&forged), payload));
        assert!(!verify_signature("", None, payload));
    }
}
