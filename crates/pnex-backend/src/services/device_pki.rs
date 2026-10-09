//! Device PKI (D153, lots L3–L4 of security-tiers.md §6 bis): the X.509
//! identity of a device on its TLS link.
//!
//! - One certificate authority per org (ECDSA P-256, 30 years), created on
//!   first use under a cluster-wide lock. Its private key is a **platform
//!   secret of the vault** (encrypted at rest, rewritten by the master key
//!   rotation); the certificate is public and lives in `org_device_cas`.
//! - Every firmware build issues a fresh device certificate (ECDSA P-256,
//!   10 years, `CN = device_id`, client-auth only). Its private key exists
//!   only in that firmware image; the registry (`device_certificates`)
//!   keeps the SHA-256 fingerprint, serial and expiry, never the key.
//! - [`verify_client_cert`] maps a certificate presented on the TLS link
//!   (forwarded by the edge, L4) to its device: fingerprint in the registry,
//!   not revoked, not expired, and signed by the device org's CA.
//!
//! The org boundary is the CA: a certificate of org A never authenticates a
//! device of org B (the chain is checked against the org of the device the
//! fingerprint belongs to).

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType, SerialNumber,
};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, UnixTime};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use sha2::{Digest, Sha256};

use crate::models::_entities::{device_certificates, org_device_cas};
use crate::services::db_lock::{ns, TenantLock};
use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::Keyring;

/// Validity of an org CA.
const CA_YEARS: i64 = 30;
/// Validity of a device certificate.
const DEVICE_YEARS: i64 = 10;

#[derive(Debug, thiserror::Error)]
pub enum PkiError {
    #[error("vault: {0}")]
    Store(#[from] StoreError),
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("certificate generation: {0}")]
    Gen(#[from] rcgen::Error),
    #[error("stored device CA of org {0} is unreadable")]
    BadCa(i64),
}

/// A device certificate and its private key, both PEM (key = PKCS#8).
#[derive(Clone)]
pub struct IssuedCert {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint_sha256: String,
}

impl std::fmt::Debug for IssuedCert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the private key.
        f.debug_struct("IssuedCert")
            .field("fingerprint_sha256", &self.fingerprint_sha256)
            .finish_non_exhaustive()
    }
}

/// Vault name of the CA key of `org_id` (platform secret).
fn ca_key_name(org_id: i64) -> String {
    format!("pnex-device-ca-org-{org_id}")
}

fn years_from_now(years: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc() + time::Duration::days(365 * years)
}

/// Lowercase hex SHA-256 of a DER certificate.
pub fn fingerprint(der: &[u8]) -> String {
    pnex_core::ota_sig::to_hex(&Sha256::digest(der))
}

/// Parameters of the CA of `org_id`. Deterministic: the same function
/// builds the CA at creation and the issuer that signs device certificates
/// (subject and key identifier must match the CA certificate exactly).
fn ca_params(org_id: i64) -> Result<CertificateParams, rcgen::Error> {
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params
        .distinguished_name
        .push(DnType::CommonName, format!("PneX device CA (org {org_id})"));
    params
        .distinguished_name
        .push(DnType::OrganizationName, "PneX");
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    Ok(params)
}

/// CA certificate (PEM) + key of `org_id`, created on first use.
async fn org_ca(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
) -> Result<(String, KeyPair), PkiError> {
    if let Some(found) = load_ca(db, ring, org_id).await? {
        return Ok(found);
    }
    let lock = TenantLock::acquire(
        db,
        ns::DEVICE_CA,
        org_id,
        std::time::Duration::from_secs(10),
    )
    .await?;
    let result = async {
        if let Some(found) = load_ca(db, ring, org_id).await? {
            return Ok(found);
        }
        let key = KeyPair::generate()?;
        let mut params = ca_params(org_id)?;
        params.not_before = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
        params.not_after = years_from_now(CA_YEARS);
        let cert = params.self_signed(&key)?;
        let secret = store::create(
            db,
            ring,
            Writer {
                org_id: None,
                user_id: None,
            },
            &ca_key_name(org_id),
            Some("Private key of the device certificate authority of an organization (D153)."),
            &key.serialize_pem(),
        )
        .await?;
        org_device_cas::ActiveModel {
            org_id: Set(org_id),
            cert_pem: Set(cert.pem()),
            key_secret_id: Set(secret.id),
            created_at: Set(chrono::Utc::now().into()),
        }
        .insert(db)
        .await?;
        tracing::info!(org_id, "device CA created");
        Ok((cert.pem(), key))
    }
    .await;
    lock.release().await;
    result
}

async fn load_ca(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
) -> Result<Option<(String, KeyPair)>, PkiError> {
    let Some(row) = org_device_cas::Entity::find_by_id(org_id).one(db).await? else {
        return Ok(None);
    };
    let key_pem = store::reveal(db, ring, None, row.key_secret_id).await?;
    let key = KeyPair::from_pem(&key_pem).map_err(|_| PkiError::BadCa(org_id))?;
    Ok(Some((row.cert_pem, key)))
}

/// Public CA certificate of `org_id`, if it exists yet.
pub async fn org_ca_cert(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Option<String>, sea_orm::DbErr> {
    Ok(org_device_cas::Entity::find_by_id(org_id)
        .one(db)
        .await?
        .map(|r| r.cert_pem))
}

/// Issues a device certificate for `device_id` (registry pk `device_pk`)
/// of `org_id` and records it. Previous certificates of the device stay
/// valid (a device still running the previous firmware keeps connecting
/// until its update); delete or revoke them to cut it off.
pub async fn issue_device_cert(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    device_pk: i64,
    device_id: &str,
) -> Result<IssuedCert, PkiError> {
    let (_ca_pem, ca_key) = org_ca(db, ring, org_id).await?;
    let issuer = Issuer::new(ca_params(org_id)?, ca_key);

    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params
        .distinguished_name
        .push(DnType::CommonName, device_id);
    params
        .distinguished_name
        .push(DnType::OrganizationName, "PneX device");
    params.subject_alt_names = vec![SanType::URI(
        format!("urn:pnex:org:{org_id}:device:{device_pk}").try_into()?,
    )];
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let mut serial = [0u8; 16];
    rand::RngExt::fill(&mut rand::rng(), &mut serial);
    serial[0] &= 0x7f; // positive INTEGER
    params.serial_number = Some(SerialNumber::from_slice(&serial));
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    let not_after = years_from_now(DEVICE_YEARS);
    params.not_after = not_after;
    let cert = params.signed_by(&key, &issuer)?;

    let fingerprint_sha256 = fingerprint(cert.der());
    device_certificates::ActiveModel {
        org_id: Set(org_id),
        device_registry_id: Set(device_pk),
        serial: Set(pnex_core::ota_sig::to_hex(&serial)),
        fingerprint_sha256: Set(fingerprint_sha256.clone()),
        not_after: Set(
            chrono::DateTime::from_timestamp(not_after.unix_timestamp(), 0)
                .unwrap_or_else(chrono::Utc::now)
                .into(),
        ),
        revoked_at: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(IssuedCert {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
        fingerprint_sha256,
    })
}

/// Revokes every live certificate of a device (credentials rotated: an
/// agent re-enrollment, a device reset). Returns how many were revoked.
pub async fn revoke_all(db: &DatabaseConnection, device_pk: i64) -> Result<u64, sea_orm::DbErr> {
    use sea_orm::sea_query::Expr;
    let res = device_certificates::Entity::update_many()
        .col_expr(
            device_certificates::Column::RevokedAt,
            Expr::value(sea_orm::Value::from(
                chrono::DateTime::<chrono::FixedOffset>::from(chrono::Utc::now()),
            )),
        )
        .filter(device_certificates::Column::DeviceRegistryId.eq(device_pk))
        .filter(device_certificates::Column::RevokedAt.is_null())
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

/// Why a presented certificate was refused (logged, never returned to the
/// client: the device only sees a close code).
#[derive(Debug, PartialEq, Eq)]
pub enum CertRefusal {
    Malformed,
    Unknown,
    Revoked,
    Expired,
    BadChain,
}

/// Device (`(org_id, device_registry_id)`) of a client certificate (PEM),
/// after checking the registry and the chain to the device org's CA.
pub async fn verify_client_cert(
    db: &DatabaseConnection,
    cert_pem: &str,
) -> Result<Result<(i64, i64), CertRefusal>, sea_orm::DbErr> {
    let Ok(der) = CertificateDer::from_pem_slice(cert_pem.as_bytes()) else {
        return Ok(Err(CertRefusal::Malformed));
    };
    let fp = fingerprint(&der);
    let Some(row) = device_certificates::Entity::find()
        .filter(device_certificates::Column::FingerprintSha256.eq(&fp))
        .one(db)
        .await?
    else {
        return Ok(Err(CertRefusal::Unknown));
    };
    if row.revoked_at.is_some() {
        return Ok(Err(CertRefusal::Revoked));
    }
    if row.not_after < chrono::Utc::now() {
        return Ok(Err(CertRefusal::Expired));
    }
    let Some(ca_pem) = org_ca_cert(db, row.org_id).await? else {
        return Ok(Err(CertRefusal::BadChain));
    };
    if !chains_to(&der, &ca_pem) {
        return Ok(Err(CertRefusal::BadChain));
    }
    Ok(Ok((row.org_id, row.device_registry_id)))
}

/// True when `cert` is a client-auth certificate signed by `ca_pem`.
fn chains_to(cert: &CertificateDer<'_>, ca_pem: &str) -> bool {
    let Ok(ca_der) = CertificateDer::from_pem_slice(ca_pem.as_bytes()) else {
        return false;
    };
    let Ok(anchor) = webpki::anchor_from_trusted_cert(&ca_der) else {
        return false;
    };
    let Ok(ee) = webpki::EndEntityCert::try_from(cert) else {
        return false;
    };
    ee.verify_for_usage(
        webpki::ALL_VERIFICATION_ALGS,
        &[anchor],
        &[],
        UnixTime::now(),
        webpki::KeyUsage::client_auth(),
        None,
        None,
    )
    .is_ok()
}
