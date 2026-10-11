//! The hub's certificate authority, when it is not one the system trusts, and
//! what a failed TLS handshake means to an operator. See
//! `docs/shared-library.md#reaching-the-hub`.

use std::error::Error;

use anyhow::{bail, Context, Result};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use rustls::CertificateError;
use serde::Serialize;
use x509_cert::der::Decode;

/// A certificate file is a few kilobytes. Anything much larger is not one,
/// and would be kept in `config.json` whole.
pub const MAX_PEM_BYTES: u64 = 256 * 1024;

/// What the Settings page shows of a chosen certificate authority.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Who the first certificate names, as its subject reads.
    pub subject: String,
    /// When the first of them to expire does, unix ms.
    pub expires_at: i64,
    /// How many certificates the text holds.
    pub count: usize,
}

/// The certificates in a PEM text. An error when it holds none.
pub fn parse(pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    if pem.len() as u64 > MAX_PEM_BYTES {
        bail!("this is too large to be a certificate");
    }
    let certs = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("this is not a PEM certificate: {e}"))?;
    if certs.is_empty() {
        bail!("this file holds no certificate");
    }
    Ok(certs)
}

/// Read a PEM text as certificates and say what they are.
pub fn summarize(pem: &str) -> Result<Summary> {
    let certs = parse(pem)?;
    let read = certs
        .iter()
        .map(|der| x509_cert::Certificate::from_der(der).context("this certificate cannot be read"))
        .collect::<Result<Vec<_>>>()?;
    let expires_at = read
        .iter()
        .map(|c| {
            c.tbs_certificate
                .validity
                .not_after
                .to_unix_duration()
                .as_millis() as i64
        })
        .min()
        .unwrap_or_default();
    Ok(Summary {
        subject: read[0].tbs_certificate.subject.to_string(),
        expires_at,
        count: read.len(),
    })
}

/// The TLS error under a failed connection, if there is one. A handshake
/// failure arrives wrapped in an I/O error, whose own `source` skips it.
fn tls_error<'a>(e: &'a (dyn Error + 'static)) -> Option<&'a rustls::Error> {
    let mut next = Some(e);
    while let Some(err) = next {
        if let Some(tls) = err.downcast_ref::<rustls::Error>() {
            return Some(tls);
        }
        next = match err
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
        {
            Some(inner) => Some(inner),
            None => err.source(),
        };
    }
    None
}

/// What a failed connection asks of the operator, where the failure is one
/// they can act on in Settings.
pub fn advice(e: &(dyn Error + 'static)) -> Option<&'static str> {
    use CertificateError as C;
    let rustls::Error::InvalidCertificate(why) = tls_error(e)? else {
        return None;
    };
    Some(match why {
        C::UnknownIssuer => {
            "the hub's certificate is not from an authority this computer trusts: \
             choose the hub's CA certificate under Shared Library"
        }
        C::NotValidForName | C::NotValidForNameContext { .. } => {
            "the hub's certificate is for another host name than the one in its address"
        }
        C::Expired | C::ExpiredContext { .. } => "the hub's certificate has expired",
        C::NotValidYet | C::NotValidYetContext { .. } => {
            "the hub's certificate is not valid yet: check this computer's clock"
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-signed test authority, valid until 2036.
    pub const TEST_CA: &str = include_str!("test_ca.pem");

    #[test]
    fn a_certificate_is_read_and_described() {
        let summary = summarize(TEST_CA).unwrap();
        assert_eq!(summary.subject, "CN=RadiodioDJ Test CA");
        assert_eq!(summary.count, 1);
        assert!(
            summary.expires_at > 2_000_000_000_000,
            "{}",
            summary.expires_at
        );
    }

    #[test]
    fn two_certificates_in_one_file_are_both_taken() {
        let both = format!("{TEST_CA}\n{TEST_CA}");
        assert_eq!(parse(&both).unwrap().len(), 2);
        assert_eq!(summarize(&both).unwrap().count, 2);
    }

    /// Whichever way it arrives: read from a file, or sent to be saved.
    #[test]
    fn text_too_large_to_be_a_certificate_is_refused() {
        let padded = format!("{TEST_CA}{}", "\n".repeat(MAX_PEM_BYTES as usize));
        assert!(parse(&padded).is_err());
        assert!(summarize(&padded).is_err());
    }

    #[test]
    fn a_file_that_is_not_a_certificate_is_refused() {
        assert!(parse("not a certificate").is_err());
        assert!(parse("").is_err());
        let key = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n";
        assert!(parse(key).is_err(), "a key is not a certificate");
    }

    #[test]
    fn an_untrusted_issuer_says_what_to_do_about_it() {
        let handshake = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer),
        );
        assert!(advice(&handshake).unwrap().contains("CA certificate"));

        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(advice(&refused), None);
    }
}
