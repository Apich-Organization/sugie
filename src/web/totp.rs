//! Two-Factor Authentication (TOTP / RFC 6238) helpers for user login.
//!
//! Configured with SHA512 and 8 digits, providing secret generation, scannable
//! QR code rendering, and 8-digit token validation (compatible with YubiKey
//! Authenticator and standard TOTP apps).

use serde::Serialize;
use totp_rs::{Algorithm, Builder, Secret};

#[derive(Debug, Clone, Serialize)]
pub struct TotpSetupData {
    /// Raw Base32 encoded secret key (for manual entry into YubiKey or authenticator).
    pub secret: String,
    /// Base32 secret formatted into 4-character blocks for readability.
    pub formatted_secret: String,
    /// Scannable QR code formatted as a PNG data URL (`data:image/png;base64,...`).
    pub qr_data_url: String,
    /// Standard otpauth:// URI.
    pub otpauth_url: String,
    /// Issuer name (from RP_NAME).
    pub issuer: String,
    /// Account name (username).
    pub account_name: String,
}

/// Generates a new TOTP secret and scannable QR code (8 digits, SHA512).
///
/// Formats the URI as:
/// `otpauth://totp/Service:account?secret=YOUR_SECRET&algorithm=SHA512&digits=8&period=30`
pub fn generate_totp_setup(issuer: &str, account_name: &str) -> Result<TotpSetupData, String> {
    let secret = Secret::generate();
    let secret_encoded = secret.to_base32();

    let clean_issuer = if issuer.trim().is_empty() {
        "Sugie".to_string()
    } else {
        issuer.trim().replace(':', "")
    };
    let clean_account = account_name.trim().replace(':', "");

    let _totp = Builder::new()
        .with_algorithm(Algorithm::SHA512)
        .with_digits(8)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(secret)
        .with_issuer(Some(clean_issuer.as_str()))
        .with_account_name(clean_account.as_str())
        .build()
        .map_err(|e| format!("Failed to initialize TOTP: {e}"))?;

    let otpauth_url = format!(
        "otpauth://totp/{clean_issuer}:{clean_account}?secret={secret_encoded}&algorithm=SHA512&digits=8&period=30"
    );
    let qr_base64 = qrcodegen_image::draw_base64(&otpauth_url)
        .map_err(|e| format!("Failed to render QR code: {e}"))?;
    let qr_data_url = format!("data:image/png;base64,{qr_base64}");

    let formatted_secret = secret_encoded
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");

    Ok(TotpSetupData {
        secret: secret_encoded,
        formatted_secret,
        qr_data_url,
        otpauth_url,
        issuer: clean_issuer,
        account_name: clean_account,
    })
}

/// Verifies an 8-digit TOTP code against a Base32 encoded secret.
/// Primary validation uses SHA512 with 8 digits, with fallbacks for SHA1/SHA256 8-digit configurations.
pub fn verify_totp(secret_encoded: &str, code: &str) -> bool {
    let clean_code = code.trim().replace(' ', "");
    let clean_secret = secret_encoded.trim().replace(' ', "").to_uppercase();

    if clean_code.is_empty() || clean_secret.is_empty() {
        return false;
    }

    let Ok(secret) = Secret::try_from_base32(&clean_secret) else {
        return false;
    };

    // 1. Primary standard: SHA512, 8 digits, 30s step, 1 step tolerance
    if let Ok(totp) = Builder::new()
        .with_algorithm(Algorithm::SHA512)
        .with_digits(8)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(secret.clone())
        .build()
    {
        if totp.check_current(&clean_code).is_some() {
            return true;
        }
    }

    // 2. Fallback: SHA1, 8 digits
    if let Ok(totp) = Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_digits(8)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(secret.clone())
        .build()
    {
        if totp.check_current(&clean_code).is_some() {
            return true;
        }
    }

    // 3. Fallback: SHA256, 8 digits
    if let Ok(totp) = Builder::new()
        .with_algorithm(Algorithm::SHA256)
        .with_digits(8)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(secret)
        .build()
    {
        if totp.check_current(&clean_code).is_some() {
            return true;
        }
    }

    false
}
