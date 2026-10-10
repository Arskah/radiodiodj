//! The statements the app runs against the hub, and nothing else: no rule
//! about what a row means lives here.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use rustls::pki_types::CertificateDer;
use rustls_platform_verifier::Verifier;
use tokio_postgres::config::{SslMode, SslNegotiation};
use tokio_postgres::{Config, Socket};
use tokio_postgres_rustls::MakeRustlsConnect;

use super::certs;
use crate::persist::config::ExternalLibraryConfig;

use super::schema::{PROTOCOL, STATION_LOCK, TABLES};
use crate::library::db::{Incoming, Outgoing};

/// Longer than any statement here should take, and short enough that a hub
/// that has stopped answering is noticed within a cycle.
const TIMEOUT: Duration = Duration::from_secs(30);

/// This machine, as the hub knows it.
#[derive(Clone, Debug)]
pub struct Machine {
    pub id: String,
    pub name: String,
}

/// What claiming the owner role came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// This machine is the owner of the library the hub holds under this id.
    Held { library_id: String },
    /// The hub held another library, or none. It was emptied and now holds
    /// this one under a new id, so everything has to be published again.
    Replaced { library_id: String },
    /// Another machine is the owner.
    Taken { by: String },
}

/// The library the hub holds.
#[derive(Debug, PartialEq, Eq)]
pub struct Station {
    pub library_id: String,
    pub protocol: i32,
    /// The machine holding the owner role, if one does.
    pub owner: Option<Owner>,
}

/// The library owner, as the hub last heard from it.
#[derive(Debug, PartialEq, Eq)]
pub struct Owner {
    pub id: String,
    pub name: String,
    /// Seconds since it last checked in.
    pub quiet_for: i64,
}

/// How to reach the hub: its address, and what this machine accepts of the
/// connection. See `docs/shared-library.md#reaching-the-hub`.
pub struct Link {
    config: Config,
    allow_unencrypted: bool,
    ca: Vec<CertificateDer<'static>>,
    /// Whether the last connection that opened was encrypted.
    encrypted: Mutex<Option<bool>>,
}

impl Link {
    pub fn new(settings: &ExternalLibraryConfig) -> Result<Self> {
        let mut config: Config = settings
            .url
            .as_deref()
            .unwrap_or_default()
            .parse()
            .context("the hub's address is not a postgresql:// connection URL")?;
        config.connect_timeout(Duration::from_secs(10));
        config.application_name("radiodiodj");
        if settings.direct_tls {
            config.ssl_negotiation(SslNegotiation::Direct);
        }
        let ca = match &settings.ca_certificate {
            Some(pem) => certs::parse(pem).context("the hub's CA certificate")?,
            None => Vec::new(),
        };
        Ok(Self {
            config,
            allow_unencrypted: settings.allow_unencrypted,
            ca,
            encrypted: Mutex::new(None),
        })
    }

    /// A link to a test database, which is addressed by more than a URL can
    /// say.
    #[cfg(test)]
    pub fn over(config: Config, allow_unencrypted: bool, ca: Vec<CertificateDer<'static>>) -> Self {
        Self {
            config,
            allow_unencrypted,
            ca,
            encrypted: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Whether the last connection that opened was encrypted, once one has.
    pub fn encrypted(&self) -> Option<bool> {
        *self.encrypted.lock()
    }

    /// TLS as this machine accepts it: a certificate from an authority the
    /// system trusts or from the station's own, for the host name asked for.
    fn tls(&self) -> Result<MakeRustlsConnect> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = Verifier::new_with_extra_roots(self.ca.iter().cloned(), provider.clone())?;
        let mut tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        // What a proxy ending TLS in front of the hub routes on, and what
        // Postgres itself insists on when the handshake comes first.
        tls.alpn_protocols = vec![b"postgresql".to_vec()];
        Ok(MakeRustlsConnect::new(tls))
    }

    /// Open a connection. Encrypted, unless the operator allowed otherwise
    /// and the hub offers nothing else: TLS is always tried first, so a
    /// connection is never plaintext because of a default.
    pub async fn connect(&self) -> Result<Hub> {
        let mut config = self.config.clone();
        config.ssl_mode(SslMode::Require);
        match timed(config.connect(self.tls()?)).await {
            Ok(opened) => return Ok(self.opened(opened, true)),
            Err(e) if !offers_no_tls(&e) => return Err(explained(e)),
            Err(_) if !self.allow_unencrypted => bail!(
                "the hub does not offer an encrypted connection: enable TLS on it, \
                 or allow an unencrypted connection under Shared Library"
            ),
            Err(_) => {}
        }
        config.ssl_mode(SslMode::Disable);
        let opened = timed(config.connect(tokio_postgres::NoTls))
            .await
            .map_err(explained)?;
        Ok(self.opened(opened, false))
    }

    fn opened<S>(
        &self,
        (client, connection): (
            tokio_postgres::Client,
            tokio_postgres::Connection<Socket, S>,
        ),
        encrypted: bool,
    ) -> Hub
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        tauri::async_runtime::spawn(async move {
            if let Err(e) = connection.await {
                log::debug!("hub connection closed: {e}");
            }
        });
        *self.encrypted.lock() = Some(encrypted);
        Hub { client }
    }
}

/// Whether a connection failed because the server has no TLS to offer —
/// the one failure an unencrypted connection is an answer to.
fn offers_no_tls(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains("server does not support TLS")
}

/// A failed connection as the operator should read it: what to do, where
/// Settings can do something, and what happened otherwise.
fn explained(e: anyhow::Error) -> anyhow::Error {
    match certs::advice(e.as_ref()) {
        Some(advice) => {
            log::debug!("hub connection refused: {e:#}");
            anyhow!(advice)
        }
        None => e.context("the hub cannot be reached"),
    }
}

/// A connection to the hub.
pub struct Hub {
    client: tokio_postgres::Client,
}

impl Hub {
    /// Create the hub's tables where they are missing. The owner's to run.
    pub async fn create_tables(&self) -> Result<()> {
        timed(self.client.batch_execute(TABLES)).await
    }

    /// Say this machine was here.
    pub async fn check_in(&self, machine: &Machine) -> Result<()> {
        timed(self.client.execute(
            "INSERT INTO hub_machines (id, name, build, seen_at) VALUES ($1, $2, $3, now()) \
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, build = excluded.build, \
                                            seen_at = excluded.seen_at",
            &[&machine.id, &machine.name, &env!("CARGO_PKG_VERSION")],
        ))
        .await?;
        Ok(())
    }

    /// Claim the owner role for the library this machine knows as
    /// `library_id`.
    ///
    /// A hub that holds a different library than this machine last published —
    /// or holds one this machine never published to — is emptied: its rows
    /// carry another database's track ids.
    pub async fn claim(&mut self, machine: &Machine, library_id: Option<&str>) -> Result<Claim> {
        let tx = timed(self.client.transaction()).await?;
        timed(tx.execute("SELECT pg_advisory_xact_lock($1)", &[&STATION_LOCK])).await?;
        let station =
            timed(tx.query_opt("SELECT library_id, protocol, owner FROM hub_station", &[])).await?;
        let fresh = || uuid::Uuid::new_v4().to_string();
        let claim = match station {
            None => {
                let id = library_id.map_or_else(fresh, str::to_owned);
                timed(tx.execute(
                    "INSERT INTO hub_station (library_id, protocol, owner) VALUES ($1, $2, $3)",
                    &[&id, &PROTOCOL, &machine.id],
                ))
                .await?;
                timed(tx.execute("DELETE FROM hub_rows", &[])).await?;
                Claim::Replaced { library_id: id }
            }
            Some(row) => {
                let held: String = row.get(0);
                let protocol: i32 = row.get(1);
                let owner: Option<String> = row.get(2);
                if protocol > PROTOCOL {
                    bail!(
                        "the hub speaks protocol {protocol} and this version speaks {PROTOCOL}: \
                         update RadiodioDJ on this machine"
                    );
                }
                match owner {
                    Some(other) if other != machine.id => {
                        let by: Option<String> = timed(
                            tx.query_opt("SELECT name FROM hub_machines WHERE id = $1", &[&other]),
                        )
                        .await?
                        .map(|r| r.get(0));
                        return Ok(Claim::Taken {
                            by: by.unwrap_or(other),
                        });
                    }
                    _ if library_id == Some(held.as_str()) => {
                        timed(tx.execute(
                            "UPDATE hub_station SET owner = $1, protocol = $2",
                            &[&machine.id, &PROTOCOL],
                        ))
                        .await?;
                        Claim::Held { library_id: held }
                    }
                    _ => {
                        let id = fresh();
                        timed(tx.execute(
                            "UPDATE hub_station SET owner = $1, protocol = $2, library_id = $3",
                            &[&machine.id, &PROTOCOL, &id],
                        ))
                        .await?;
                        timed(tx.execute("DELETE FROM hub_rows", &[])).await?;
                        Claim::Replaced { library_id: id }
                    }
                }
            }
        };
        timed(tx.commit()).await?;
        Ok(claim)
    }

    /// The library the hub holds, or `None` for a hub no owner has published
    /// to — its tables included, which only an owner creates.
    pub async fn station(&self) -> Result<Option<Station>> {
        let row = match tokio::time::timeout(
            TIMEOUT,
            self.client.query_opt(
                "SELECT s.library_id, s.protocol, s.owner, m.name, \
                        extract(epoch FROM now() - m.seen_at)::bigint \
                 FROM hub_station s LEFT JOIN hub_machines m ON m.id = s.owner",
                &[],
            ),
        )
        .await
        {
            Err(_) => bail!("the hub did not answer in {} s", TIMEOUT.as_secs()),
            Ok(Err(e)) if e.code() == Some(&tokio_postgres::error::SqlState::UNDEFINED_TABLE) => {
                return Ok(None)
            }
            Ok(found) => found?,
        };
        Ok(row.map(|r| {
            let owner: Option<String> = r.get(2);
            Station {
                library_id: r.get(0),
                protocol: r.get(1),
                owner: owner.map(|id| Owner {
                    name: r.get::<_, Option<String>>(3).unwrap_or_else(|| id.clone()),
                    quiet_for: r.get::<_, Option<i64>>(4).unwrap_or(i64::MAX),
                    id,
                }),
            }
        }))
    }

    /// Give up the owner role, if this machine holds it. A machine that has
    /// become a studio would otherwise be named as an owner that never
    /// publishes.
    pub async fn release(&self, machine: &Machine) -> Result<()> {
        timed(self.client.execute(
            "UPDATE hub_station SET owner = NULL WHERE owner = $1",
            &[&machine.id],
        ))
        .await?;
        Ok(())
    }

    /// Up to `limit` rows after revision `after`, oldest first. `skip` leaves
    /// out one machine's own rows: an owner would otherwise be handed back the
    /// whole library it published.
    pub async fn fetch(
        &self,
        after: i64,
        limit: i64,
        skip: Option<&Machine>,
    ) -> Result<Vec<Incoming>> {
        let skip = skip.map(|m| m.id.as_str());
        let rows = timed(self.client.query(
            "SELECT kind, key, rev, deleted, doc, waveform, levels, edited_at, machine \
             FROM hub_rows \
             WHERE rev > $1 AND ($3::text IS NULL OR machine <> $3) ORDER BY rev LIMIT $2",
            &[&after, &limit, &skip],
        ))
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Incoming {
                kind: r.get(0),
                key: r.get(1),
                rev: r.get(2),
                deleted: r.get(3),
                doc: r.get(4),
                waveform: r.get(5),
                levels: r.get(6),
                edited_at: r.get(7),
                machine: r.get(8),
            })
            .collect())
    }

    /// Send this machine's groups, all of them or none.
    ///
    /// The library's paths and tracks are the owner's: they are written as
    /// they come, and only while this machine holds the role. Everything else
    /// is anyone's, and is written only over an earlier save — the later one
    /// wins, by the saving machines' clocks, with the machine id to settle a
    /// tie. A group that loses is not an error; the winner comes back with the
    /// next pull.
    pub async fn send(
        &mut self,
        machine: &Machine,
        groups: &[Outgoing],
        owner: bool,
    ) -> Result<()> {
        let tx = timed(self.client.transaction()).await?;
        timed(tx.execute("SELECT pg_advisory_xact_lock($1)", &[&STATION_LOCK])).await?;
        if owner {
            let holder: Option<String> = timed(tx.query_opt("SELECT owner FROM hub_station", &[]))
                .await?
                .and_then(|r| r.get(0));
            if holder.as_deref() != Some(machine.id.as_str()) {
                return Err(anyhow!("another machine has taken the owner role"));
            }
        } else if groups
            .iter()
            .any(|g| OWNER_KINDS.contains(&g.kind.as_str()))
        {
            bail!("only the library owner publishes paths and tracks");
        }
        let upsert = timed(tx.prepare(
            "INSERT INTO hub_rows (kind, key, rev, machine, edited_at, deleted, doc, waveform, levels) \
             VALUES ($1, $2, nextval('hub_rev'), $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (kind, key) DO UPDATE SET \
               rev = excluded.rev, machine = excluded.machine, edited_at = excluded.edited_at, \
               deleted = excluded.deleted, doc = excluded.doc, waveform = excluded.waveform, \
               levels = excluded.levels \
             WHERE hub_rows.kind IN ('root', 'track') \
                OR (excluded.edited_at, excluded.machine) > (hub_rows.edited_at, hub_rows.machine)",
        ))
        .await?;
        // What was part of a purged track goes with it: nothing could apply
        // it again, and a studio would park it forever.
        let forget = timed(tx.prepare(
            "DELETE FROM hub_rows \
             WHERE (kind IN ('cue', 'hidden') AND key = $1) \
                OR (kind = 'edit' AND key LIKE $1 || ':%')",
        ))
        .await?;
        for g in groups {
            timed(tx.execute(
                &upsert,
                &[
                    &g.kind,
                    &g.key,
                    &machine.id,
                    &g.edited_at,
                    &g.deleted,
                    &g.doc,
                    &g.waveform,
                    &g.levels,
                ],
            ))
            .await?;
            if g.kind == "track" && g.deleted {
                timed(tx.execute(&forget, &[&g.key])).await?;
            }
        }
        timed(tx.commit()).await
    }
}

/// The kinds only the owner writes.
const OWNER_KINDS: &[&str] = &["root", "track"];

/// One hub call, bounded: a hub that stops answering is an error, not a wait.
async fn timed<T, E>(call: impl std::future::Future<Output = Result<T, E>>) -> Result<T>
where
    E: Into<anyhow::Error>,
{
    match tokio::time::timeout(TIMEOUT, call).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => bail!("the hub did not answer in {} s", TIMEOUT.as_secs()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plain test database, as [`super::super::worker`]'s tests reach it.
    fn plain() -> Option<Config> {
        Some(std::env::var("RADIODIODJ_TEST_HUB").ok()?.parse().unwrap())
    }

    /// A test database that offers TLS under a private authority:
    /// `RADIODIODJ_TEST_HUB_TLS` is its URL and `RADIODIODJ_TEST_HUB_CA` the
    /// path of the authority's certificate. `None` without both.
    fn with_tls() -> Option<(Config, Vec<CertificateDer<'static>>)> {
        let url = std::env::var("RADIODIODJ_TEST_HUB_TLS").ok()?;
        let ca = std::fs::read_to_string(std::env::var("RADIODIODJ_TEST_HUB_CA").ok()?).unwrap();
        Some((url.parse().unwrap(), certs::parse(&ca).unwrap()))
    }

    fn refusal(outcome: Result<Hub>) -> String {
        match outcome {
            Ok(_) => panic!("the connection opened"),
            Err(e) => format!("{e:#}"),
        }
    }

    #[tokio::test]
    async fn a_private_authority_is_trusted_once_it_is_named() {
        let Some((config, ca)) = with_tls() else {
            return;
        };
        let link = Link::over(config, false, ca);

        link.connect().await.unwrap();

        assert_eq!(link.encrypted(), Some(true));
    }

    #[tokio::test]
    async fn a_certificate_from_an_unknown_authority_is_refused_with_what_to_do() {
        let Some((config, _)) = with_tls() else {
            return;
        };
        let link = Link::over(config, false, Vec::new());

        let why = refusal(link.connect().await);

        assert!(why.contains("CA certificate"), "{why}");
        assert_eq!(link.encrypted(), None);
    }

    /// Allowing plaintext is an answer to a hub with no TLS, not to one whose
    /// certificate cannot be trusted.
    #[tokio::test]
    async fn an_untrusted_certificate_is_not_answered_with_plaintext() {
        let Some((config, _)) = with_tls() else {
            return;
        };
        let link = Link::over(config, true, Vec::new());

        let why = refusal(link.connect().await);

        assert!(why.contains("CA certificate"), "{why}");
    }

    #[tokio::test]
    async fn a_certificate_for_another_name_is_refused() {
        let Some((config, ca)) = with_tls() else {
            return;
        };
        // The same server, asked for by a name its certificate does not carry.
        let mut renamed = Config::new();
        renamed
            .host("hub.invalid")
            .hostaddr([127, 0, 0, 1].into())
            .port(config.get_ports()[0])
            .user(config.get_user().unwrap())
            .dbname(config.get_dbname().unwrap());
        if let Some(password) = config.get_password() {
            renamed.password(password);
        }
        let link = Link::over(renamed, false, ca);

        let why = refusal(link.connect().await);

        assert!(why.contains("another host name"), "{why}");
    }

    #[tokio::test]
    async fn a_hub_without_tls_is_refused_unless_plaintext_is_allowed() {
        let Some(config) = plain() else { return };

        let strict = Link::over(config.clone(), false, Vec::new());
        let why = refusal(strict.connect().await);
        assert!(why.contains("allow an unencrypted connection"), "{why}");

        let lenient = Link::over(config, true, Vec::new());
        lenient.connect().await.unwrap();
        assert_eq!(lenient.encrypted(), Some(false));
    }

    /// TLS is tried first even when plaintext is allowed, so allowing it
    /// never costs an encryption the hub offers.
    #[tokio::test]
    async fn allowing_plaintext_still_encrypts_where_the_hub_can() {
        let Some((config, ca)) = with_tls() else {
            return;
        };
        let link = Link::over(config, true, ca);

        link.connect().await.unwrap();

        assert_eq!(link.encrypted(), Some(true));
    }

    /// Postgres takes a handshake that comes first from version 17 on; an
    /// older test server has nothing to say about this.
    #[tokio::test]
    async fn the_handshake_can_come_first() {
        let Some((mut config, ca)) = with_tls() else {
            return;
        };
        let probe = Link::over(config.clone(), false, ca.clone());
        let version: String = probe
            .connect()
            .await
            .unwrap()
            .client
            .query_one("SHOW server_version_num", &[])
            .await
            .unwrap()
            .get(0);
        if version.parse::<i32>().unwrap() < 170_000 {
            return;
        }
        config.ssl_negotiation(SslNegotiation::Direct);
        let link = Link::over(config, false, ca);

        link.connect().await.unwrap();

        assert_eq!(link.encrypted(), Some(true));
    }

    #[test]
    fn an_address_that_is_not_a_url_is_said_to_be_one() {
        let settings = ExternalLibraryConfig {
            url: Some("hub.example.org".into()),
            ..Default::default()
        };
        let why = Link::new(&settings)
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap();
        assert!(why.contains("connection URL"), "{why}");
    }

    #[test]
    fn a_ca_certificate_that_is_not_one_is_refused_before_connecting() {
        let settings = ExternalLibraryConfig {
            url: Some("postgresql://hub/x".into()),
            ca_certificate: Some("nonsense".into()),
            ..Default::default()
        };
        assert!(Link::new(&settings).is_err());
    }
}
