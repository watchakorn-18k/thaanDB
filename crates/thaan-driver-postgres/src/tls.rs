//! TLS for PostgreSQL connections (rustls with the `ring` provider). The `sslmode`
//! values follow libpq's documentation:
//!
//! - `disable`: plain TCP.
//! - `allow`, `prefer` (the default): TLS when the server offers it, plain otherwise.
//!   Like libpq, the certificate is not checked.
//! - `require`: TLS or fail. The certificate is not checked (libpq does the same unless a
//!   root certificate is given, in which case it behaves as `verify-ca`).
//! - `verify-ca`: TLS, the chain must lead to a trusted root (the system store, plus
//!   `sslrootcert` if set). The host name is not checked.
//! - `verify-full`: as `verify-ca`, and the certificate must name the host.

use postgres::config::SslMode;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use std::sync::Arc;
use thaan_core::{Error, Result};
use tokio_postgres_rustls::MakeRustlsConnect;

/// The `sslmode` values, in the order the connection form lists them.
pub const MODES: &[&str] = &[
    "prefer",
    "disable",
    "allow",
    "require",
    "verify-ca",
    "verify-full",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    /// Encrypt only.
    None,
    /// Trusted chain, any host name.
    Ca,
    /// Trusted chain and matching host name.
    Full,
}

/// What the driver passes to `postgres::Config`: the negotiation mode and how strictly to
/// check the server's certificate.
pub fn parse_mode(mode: Option<&str>, root_cert: Option<&str>) -> Result<(SslMode, Verify)> {
    let has_root = root_cert.is_some_and(|p| !p.trim().is_empty());
    Ok(
        match mode
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or("prefer")
        {
            "disable" => (SslMode::Disable, Verify::None),
            "allow" | "prefer" => (SslMode::Prefer, Verify::None),
            "require" if has_root => (SslMode::Require, Verify::Ca),
            "require" => (SslMode::Require, Verify::None),
            "verify-ca" => (SslMode::Require, Verify::Ca),
            "verify-full" => (SslMode::Require, Verify::Full),
            other => {
                return Err(Error::InvalidParams(format!(
                    "sslmode must be one of {}, not \"{other}\"",
                    MODES.join(", ")
                )))
            }
        },
    )
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The TLS connector for `postgres::Config::connect`.
pub fn connector(verify: Verify, root_cert: Option<&str>) -> Result<MakeRustlsConnect> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Database(format!("TLS setup: {e}")))?;
    let config = match verify {
        Verify::None => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCheck(provider)))
            .with_no_client_auth(),
        Verify::Ca | Verify::Full => {
            let roots = Arc::new(roots(root_cert)?);
            let webpki = WebPkiServerVerifier::builder_with_provider(roots, provider)
                .build()
                .map_err(|e| Error::Database(format!("TLS setup: {e}")))?;
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Checked {
                    inner: webpki,
                    check_name: verify == Verify::Full,
                }))
                .with_no_client_auth()
        }
    };
    Ok(MakeRustlsConnect::new(config))
}

/// The system's trusted roots, plus the PEM file the user named (for example a cloud
/// provider's CA bundle, which is usually not in the system store).
fn roots(root_cert: Option<&str>) -> Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        // A system certificate rustls cannot parse is skipped, as browsers do.
        let _ = store.add(cert);
    }
    if let Some(path) = root_cert.map(str::trim).filter(|p| !p.is_empty()) {
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
            .map_err(|e| Error::InvalidParams(format!("sslrootcert {path}: {e}")))?
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| Error::InvalidParams(format!("sslrootcert {path}: {e}")))?;
        if certs.is_empty() {
            return Err(Error::InvalidParams(format!(
                "sslrootcert {path}: no PEM certificates in the file"
            )));
        }
        for cert in certs {
            store
                .add(cert)
                .map_err(|e| Error::InvalidParams(format!("sslrootcert {path}: {e}")))?;
        }
    }
    if store.is_empty() {
        return Err(Error::InvalidParams(
            "no trusted root certificates: set sslrootcert to the server's CA file".into(),
        ));
    }
    Ok(store)
}

/// Encrypts without checking who the server is (`prefer` / `require`). Handshake
/// signatures are still verified, so the session key belongs to the certificate shown.
#[derive(Debug)]
struct NoCheck(Arc<CryptoProvider>);

impl ServerCertVerifier for NoCheck {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// The normal web-PKI check; for `verify-ca` a name mismatch alone is accepted.
#[derive(Debug)]
struct Checked {
    inner: Arc<WebPkiServerVerifier>,
    check_name: bool,
}

impl ServerCertVerifier for Checked {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        match self
            .inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
        {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) if !self.check_name => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_follow_libpq() {
        let m = |s: Option<&str>, r| parse_mode(s, r).map(|(_, v)| v).ok();
        assert_eq!(m(None, None), Some(Verify::None));
        assert_eq!(m(Some("require"), None), Some(Verify::None));
        assert_eq!(m(Some("require"), Some("/ca.pem")), Some(Verify::Ca));
        assert_eq!(m(Some("verify-ca"), None), Some(Verify::Ca));
        assert_eq!(m(Some("verify-full"), None), Some(Verify::Full));
        assert!(matches!(
            parse_mode(Some("disable"), None),
            Ok((SslMode::Disable, _))
        ));
        assert!(parse_mode(Some("sometimes"), None).is_err());
    }

    #[test]
    fn connectors_build() {
        assert!(connector(Verify::None, None).is_ok());
        assert!(connector(Verify::Full, Some("/no/such/ca.pem")).is_err());
    }
}
