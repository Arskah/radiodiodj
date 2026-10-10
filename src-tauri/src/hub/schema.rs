//! The hub's tables. See `docs/shared-library.md#the-hub`.

/// What this build speaks. A hub on a later protocol is left alone.
pub const PROTOCOL: i32 = 1;

/// The transaction-level advisory lock every hub writer takes, so revisions
/// are handed out in commit order.
pub const STATION_LOCK: i64 = 0x5244_4a48;

/// Created by the owner, and safe to run against a hub that already has them.
pub const TABLES: &str = "
CREATE TABLE IF NOT EXISTS hub_station (
  id         boolean PRIMARY KEY DEFAULT true CHECK (id),
  library_id text    NOT NULL,
  protocol   integer NOT NULL,
  owner      text,
  settings   jsonb   NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS hub_machines (
  id      text PRIMARY KEY,
  name    text NOT NULL,
  build   text NOT NULL,
  seen_at timestamptz NOT NULL
);

CREATE SEQUENCE IF NOT EXISTS hub_rev;

CREATE TABLE IF NOT EXISTS hub_rows (
  kind      text    NOT NULL,
  key       text    NOT NULL,
  rev       bigint  NOT NULL,
  machine   text    NOT NULL,
  edited_at bigint  NOT NULL,
  deleted   boolean NOT NULL DEFAULT false,
  doc       jsonb,
  waveform  bytea,
  levels    bytea,
  PRIMARY KEY (kind, key)
);

CREATE INDEX IF NOT EXISTS hub_rows_rev ON hub_rows (rev);
";
