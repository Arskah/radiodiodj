//! The statements the app runs against the hub, and nothing else: no rule
//! about what a row means lives here.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use tokio_postgres::Config;
use tokio_postgres_rustls::MakeRustlsConnect;

use super::schema::{PROTOCOL, STATION_LOCK, TABLES};
use crate::library::db::Outgoing;

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

/// A connection to the hub.
pub struct Hub {
    client: tokio_postgres::Client,
}

impl Hub {
    /// Whether TLS is used is the URL's `sslmode` to say; the connector is
    /// there either way.
    pub async fn connect_with(mut config: Config) -> Result<Self> {
        config.connect_timeout(Duration::from_secs(10));
        config.application_name("radiodiodj");
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?;
        let tls = {
            use rustls_platform_verifier::BuilderVerifierExt;
            tls.with_platform_verifier()?.with_no_client_auth()
        };
        let (client, connection) = timed(config.connect(MakeRustlsConnect::new(tls))).await?;
        tauri::async_runtime::spawn(async move {
            if let Err(e) = connection.await {
                log::debug!("hub connection closed: {e}");
            }
        });
        Ok(Self { client })
    }

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

    /// Publish the owner's groups: all of them, or none if this machine is no
    /// longer the owner.
    pub async fn publish(&mut self, machine: &Machine, groups: &[Outgoing]) -> Result<()> {
        let tx = timed(self.client.transaction()).await?;
        timed(tx.execute("SELECT pg_advisory_xact_lock($1)", &[&STATION_LOCK])).await?;
        let owner: Option<String> = timed(tx.query_opt("SELECT owner FROM hub_station", &[]))
            .await?
            .and_then(|r| r.get(0));
        if owner.as_deref() != Some(machine.id.as_str()) {
            return Err(anyhow!("another machine has taken the owner role"));
        }
        let upsert = timed(tx.prepare(
            "INSERT INTO hub_rows (kind, key, rev, machine, edited_at, deleted, doc, waveform, levels) \
             VALUES ($1, $2, nextval('hub_rev'), $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (kind, key) DO UPDATE SET \
               rev = excluded.rev, machine = excluded.machine, edited_at = excluded.edited_at, \
               deleted = excluded.deleted, doc = excluded.doc, waveform = excluded.waveform, \
               levels = excluded.levels",
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
        }
        timed(tx.commit()).await
    }
}

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
