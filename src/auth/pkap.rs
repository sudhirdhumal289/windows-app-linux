//! Microsoft Public Key Authentication Protocol (MS-PKAP) challenge handler.
//!
//! Handles `urn:http-auth:PKeyAuth` challenges by constructing and signing
//! JSON Web Tokens (JWT) using the device certificate and RSA private key,
//! allowing native authentication on tenants requiring device compliance.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::Utc;
use log::debug;
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Serialize)]
struct PKeyAuthHeader<'a> {
    alg: &'a str,
    typ: &'a str,
    x5c: Vec<&'a str>,
}

#[derive(Serialize)]
struct PKeyAuthPayload<'a> {
    aud: &'a str,
    nonce: &'a str,
    iat: i64,
}

pub struct DeviceIdentity {
    cert_b64: String,
    key_pair: RsaKeyPair,
}

impl DeviceIdentity {
    /// Loads the device certificate and private key from standard config paths:
    /// `~/.config/windows-app-linux/device_cert.pem` (or `.der`)
    /// `~/.config/windows-app-linux/device_key.pk8` (or `.pem` / `.key`)
    pub fn try_load_default() -> Option<Self> {
        let config_dir = crate::config::AppConfig::config_dir();
        Self::try_load_from_dir(&config_dir)
    }

    /// Loads device credentials from a specified directory.
    pub fn try_load_from_dir(dir: &Path) -> Option<Self> {
        let cert_paths = [
            dir.join("device_cert.pem"),
            dir.join("device_cert.der"),
            dir.join("device.pem"),
            dir.join("device.crt"),
        ];

        let key_paths = [
            dir.join("device_key.pk8"),
            dir.join("device_key.der"),
            dir.join("device_key.pem"),
            dir.join("device.key"),
        ];

        let cert_path = cert_paths.iter().find(|p| p.exists())?;
        let key_path = key_paths.iter().find(|p| p.exists())?;

        debug!("Loaded device certificate and key from config directory");
        Self::load(cert_path, key_path).ok()
    }

    /// Loads and parses the certificate and RSA private key.
    pub fn load(cert_path: &Path, key_path: &Path) -> Result<Self, String> {
        let cert_raw = fs::read(cert_path)
            .map_err(|e| format!("Failed to read certificate {:?}: {e}", cert_path))?;
        let key_raw = fs::read(key_path)
            .map_err(|e| format!("Failed to read private key {:?}: {e}", key_path))?;

        // Parse certificate to raw DER base64 string
        let cert_b64 = if cert_raw.starts_with(b"-----BEGIN") {
            let pem_str = String::from_utf8_lossy(&cert_raw);
            let mut b64 = String::new();
            let mut capturing = false;
            for line in pem_str.lines() {
                if line.starts_with("-----BEGIN") {
                    capturing = true;
                    continue;
                }
                if line.starts_with("-----END") {
                    break;
                }
                if capturing {
                    b64.push_str(line.trim());
                }
            }
            if b64.is_empty() {
                return Err("Failed to extract certificate from PEM".to_string());
            }
            b64
        } else {
            STANDARD.encode(&cert_raw)
        };

        // Parse private key (PKCS#8 DER format required by ring)
        let pkcs8_der = if key_raw.starts_with(b"-----BEGIN") {
            let pem_str = String::from_utf8_lossy(&key_raw);
            let mut b64 = String::new();
            let mut capturing = false;
            for line in pem_str.lines() {
                if line.starts_with("-----BEGIN") {
                    capturing = true;
                    continue;
                }
                if line.starts_with("-----END") {
                    break;
                }
                if capturing {
                    b64.push_str(line.trim());
                }
            }
            STANDARD
                .decode(b64.as_bytes())
                .map_err(|e| format!("Failed to base64-decode private key: {e}"))?
        } else {
            key_raw
        };

        let key_pair = match RsaKeyPair::from_pkcs8(&pkcs8_der) {
            Ok(kp) => kp,
            Err(_) => {
                let pkcs8_wrapped = wrap_pkcs1_in_pkcs8(&pkcs8_der);
                RsaKeyPair::from_pkcs8(&pkcs8_wrapped)
                    .map_err(|e| format!("Failed to parse RSA key (tried PKCS#8 and PKCS#1): {e:?}"))?
            }
        };

        Ok(Self { cert_b64, key_pair })
    }

    /// Generates an MS-PKAP PKeyAuth signed JWT and returns the Authorization header value.
    pub fn sign_challenge(
        &self,
        submit_url: &str,
        nonce: &str,
        context: &str,
    ) -> Result<String, String> {
        let header = PKeyAuthHeader {
            alg: "RS256",
            typ: "JWT",
            x5c: vec![&self.cert_b64],
        };

        let payload = PKeyAuthPayload {
            aud: submit_url,
            nonce,
            iat: Utc::now().timestamp(),
        };

        let header_json = serde_json::to_string(&header)
            .map_err(|e| format!("Header serialization error: {e}"))?;
        let payload_json = serde_json::to_string(&payload)
            .map_err(|e| format!("Payload serialization error: {e}"))?;

        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let message = format!("{header_b64}.{payload_b64}");

        let rng = SystemRandom::new();
        let mut signature = vec![0u8; self.key_pair.public().modulus_len()];

        self.key_pair
            .sign(&RSA_PKCS1_SHA256, &rng, message.as_bytes(), &mut signature)
            .map_err(|e| format!("RSA signing failed: {e:?}"))?;

        let sig_b64 = URL_SAFE_NO_PAD.encode(&signature);
        let jwt = format!("{message}.{sig_b64}");

        debug!("Generated PKeyAuth JWT proof for device challenge");

        let auth_header = if context.is_empty() {
            format!(r#"PKeyAuth AuthToken="{jwt}", Version="1.0""#)
        } else {
            format!(r#"PKeyAuth AuthToken="{jwt}", Context="{context}", Version="1.0""#)
        };

        Ok(auth_header)
    }
}

fn wrap_pkcs1_in_pkcs8(pkcs1: &[u8]) -> Vec<u8> {
    let rsa_algo_id: [u8; 15] = [
        0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
    ];
    let mut priv_key_octets = Vec::new();
    encode_asn1_length(&mut priv_key_octets, 0x04, pkcs1.len());
    priv_key_octets.extend_from_slice(pkcs1);

    let mut body = Vec::new();
    body.extend_from_slice(&[0x02, 0x01, 0x00]);
    body.extend_from_slice(&rsa_algo_id);
    body.extend_from_slice(&priv_key_octets);

    let mut out = Vec::new();
    encode_asn1_length(&mut out, 0x30, body.len());
    out.extend_from_slice(&body);
    out
}

fn encode_asn1_length(out: &mut Vec<u8>, tag: u8, len: usize) {
    out.push(tag);
    if len < 128 {
        out.push(len as u8);
    } else if len < 256 {
        out.push(0x81);
        out.push(len as u8);
    } else if len < 65536 {
        out.push(0x82);
        out.push((len >> 8) as u8);
        out.push((len & 0xff) as u8);
    } else {
        out.push(0x83);
        out.push((len >> 16) as u8);
        out.push(((len >> 8) & 0xff) as u8);
        out.push((len & 0xff) as u8);
    }
}
