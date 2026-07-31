use pgrx::bgworkers::*;
use pgrx::prelude::*;
use std::time::Duration;

const NAME: &'static str = "pg_perishable";
const PG_ID_MAX_LEN: usize = 63;

::pgrx::pg_module_magic!(name, version);

//TODO: remove
#[pg_extern]
fn hello_pg_perishable() -> &'static str {
    "Hello, pg_perishable"
}

// metadata table - one row per registered TTL policy
extension_sql!(
    r#"
    CREATE SCHEMA IF NOT EXISTS pg_perishable;

    CREATE TABLE pg_perishable.policies (
        id                          SERIAL PRIMARY KEY,
        table_name                  regclass    NOT NULL,   -- validated: must exist
        age_column                  name        NOT NULL,   -- the timestamp column
        max_age_seconds             bigint      NOT NULL CHECK (max_age_seconds > 0),
        is_soft_delete_enabled      boolean     NOT NULL DEFAULT false,

        is_enabled          boolean     NOT NULL DEFAULT true,
        inserted_at         timestamptz NOT NULL DEFAULT now(),

        UNIQUE (table_name, age_column)
    );
    "#,
    name = "pg_perishable_policies_table",
);

fn ensure_age_column_index(
    client: &mut spi::SpiClient,
    table_name: &str,
    age_column: &str,
) -> Result<(), spi::Error> {
    let index_name = format!(
        "pg_perishable_age_idx_{}_{}",
        table_name.replace('.', "_"),
        age_column
    );

    //for unicode, to avoid truncation of a character in the middle
    let index_name = if index_name.len() > PG_ID_MAX_LEN {
        let mut truncated = index_name.clone();
        while truncated.len() > PG_ID_MAX_LEN {
            truncated.pop();
        }
        truncated
    } else {
        index_name
    };

    let query_text = client
        .select(
            "SELECT format(
                'CREATE INDEX IF NOT EXISTS %I ON %s (%I)',
                $1, $2::regclass, $3
             )",
            None,
            &[
                index_name.as_str().into(),
                table_name.into(),
                age_column.into(),
            ],
        )?
        .first()
        .get_one::<String>()?
        .expect("format() should not return NULL");

    client.update(&query_text, None, &[])?;
    Ok(())
}

#[pg_extern]
fn pg_perishable_create_row_delete_policy(
    table_name: &str,
    age_column: &str,
    max_age_seconds: i64,
    is_soft_delete_enabled: default!(bool, false),
) -> Result<i64, spi::Error> {
    Spi::connect_mut(|client| {
        ensure_age_column_index(client, table_name, age_column)?;

        // a row for per policy
        // casting table_name::regclass ==> validation has been shifted to Postgres itself
        let q = format!(
            "INSERT INTO {}.policies (table_name, age_column, max_age_seconds, is_soft_delete_enabled)
                VALUES ($1::regclass, $2, $3, $4) RETURNING id", NAME
        );

        let row = client.update(
            &q,
            None,
            &[
                table_name.into(),
                age_column.into(),
                max_age_seconds.into(),
                is_soft_delete_enabled.into(),
            ],
        )?;

        row.first()
            .get_one::<i64>()
            .map(|opt| opt.expect("RETURNING id should never be NULL"))
    })
}

#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    BackgroundWorkerBuilder::new("pg_perishable sweeper")
        .set_function(&format!("{}_worker_main", NAME)) // the name of fn to run in the worker
        .set_library(NAME) // the name of *.so
        .set_restart_time(Some(Duration::from_secs(5))) // auto-restart if it crashes
        .enable_spi_access()
        .load();
}

// worker loop
#[pg_guard]
extern "C-unwind" fn pg_perishable_worker_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
    BackgroundWorker::connect_worker_to_spi(Some("postgres"), None);

    log!("pg_perishable background sweeper started");

    while BackgroundWorker::wait_latch(Some(Duration::from_secs(60))) {
        if BackgroundWorker::sighup_received() {
            // reload config if you add GUCs later
        }

        BackgroundWorker::transaction(|| {
            Spi::connect_mut(|client| {
                // FIXME
                let _ = client.update(
                    "SELECT 1", // TODO: placeholder for now
                    None,
                    &[],
                );
            });
        });
    }

    log!("pg_perishable background sweeper shutting down");
}

struct Policy {
    id: i32,
    table_name: String,
    age_column: String,
    max_age_seconds: i64,
    mode: String,
    target_column: Option<String>,
    replacement_text: Option<String>,
}

fn quote_ident(client: &mut spi::SpiClient, s: &str) -> Result<String, spi::Error> {
    client
        .select("SELECT quote_ident($1)", None, &[s.into()])?
        .first()
        .get_one::<String>()
        .map(|opt| opt.unwrap())
}

fn quote_literal_opt(client: &mut spi::SpiClient, s: Option<&str>) -> Result<String, spi::Error> {
    client
        .select("SELECT quote_nullable($1)", None, &[s.into()])?
        .first()
        .get_one::<String>()
        .map(|opt| opt.unwrap())
}

fn regclass_text(client: &mut spi::SpiClient, table_name: &str) -> Result<String, spi::Error> {
    client
        .select(
            "SELECT format('%s', $1::regclass)",
            None,
            &[table_name.into()],
        )?
        .first()
        .get_one::<String>()
        .map(|opt| opt.unwrap())
}

// whether there may be UNIQUE or PRIMARY KEY constraint on a column;
// if there is, a random postfix will have to be attached to a redacted value
fn has_unique_constraint(table_name: &str, column_name: &str) -> Result<bool, spi::Error> {
    Spi::connect(|client| {
        client
            .select(
                "SELECT EXISTS (
                    SELECT 1
                    FROM pg_catalog.pg_constraint c
                    JOIN pg_catalog.pg_attribute a
                        ON a.attrelid = c.conrelid
                       AND a.attnum = ANY(c.conkey)
                    WHERE c.conrelid = $1::regclass
                      AND a.attname = $2
                      AND c.contype IN ('u', 'p')
                 )",
                None,
                &[table_name.into(), column_name.into()],
            )?
            .first()
            .get_one::<bool>()
            .map(|opt| opt.unwrap_or(false))
    })
}

#[pg_extern]
fn pg_perishable_create_cell_redact_policy(
    table_name: &str,
    age_column: &str,
    target_column: &str,
    max_age_seconds: i64,
    replacement_text: default!(Option<&str>, "NULL"),
) -> Result<i64, spi::Error> {
    match lookup_column(table_name, age_column)? {
        None => error!(
            "pg_perishable: column \"{}\" does not exist on table \"{}\"",
            age_column, table_name
        ),
        Some(info) if !is_timestamp_like(&info.type_name) => error!(
            "pg_perishable: column \"{}\" is type {}, expected timestamp or timestamptz",
            age_column, info.type_name
        ),
        Some(_) => {}
    }

    match lookup_column(table_name, target_column)? {
        None => error!(
            "pg_perishable: target column \"{}\" does not exist on table \"{}\"",
            target_column, table_name
        ),
        Some(info) => match replacement_text {
            None => {
                if info.is_not_null {
                    error!(
                        "pg_perishable: cannot redact \"{}\" to NULL — column has a NOT NULL constraint. \
                         Provide a non-NULL replacement_text instead.",
                        target_column
                    );
                }
            }
            Some(_) => {
                if !is_text_like(&info.type_name) {
                    error!(
                        "pg_perishable: cannot redact \"{}\" (type {}) with a text placeholder. \
                         Non-text columns only support redaction to NULL.",
                        target_column, info.type_name
                    );
                }
            }
        },
    }

    Spi::connect_mut(|client| {
        ensure_age_column_index(client, table_name, age_column)?;

        let row = client.update(
            "INSERT INTO pg_perishable.policies
                (table_name, age_column, max_age_seconds, mode, target_column, replacement_text)
             VALUES ($1::regclass, $2, $3, 'redact_column', $4, $5)
             RETURNING id",
            None,
            &[
                table_name.into(),
                age_column.into(),
                max_age_seconds.into(),
                target_column.into(),
                replacement_text.into(),
            ],
        )?;
        row.first()
            .get_one::<i64>()
            .map(|opt| opt.expect("RETURNING id should never be NULL"))
    })
}

fn sweep_redact(policy: &Policy) -> Result<(), spi::Error> {
    let target = policy
        .target_column
        .as_deref()
        .expect("redact_column policies must have target_column set");

    Spi::connect_mut(|client| {
        let table_sql = regclass_text(client, &policy.table_name)?;
        let target_ident = quote_ident(client, target)?;
        let age_ident = quote_ident(client, &policy.age_column)?;
        let replacement_literal = quote_literal_opt(client, policy.replacement_text.as_deref())?;
        let needs_suffix = has_unique_constraint(&policy.table_name, target)?;

        let set_clause = if needs_suffix && policy.replacement_text.is_some() {
            format!("{target_ident} = {replacement_literal} || '_' || gen_random_uuid()::text")
        } else {
            format!("{target_ident} = {replacement_literal}")
        };

        let query_text = format!(
            "UPDATE {table_sql} SET {set_clause} WHERE {age_ident} < now() - interval '{} seconds'",
            policy.max_age_seconds
        );

        let result = client.update(&query_text, None, &[])?;
        let rows_affected = result.len() as i64;

        //TODO
        // log_sweep(client, policy, rows_affected)?;

        Ok(())
    })
}

struct ColumnInfo {
    type_name: String,
    is_not_null: bool,
}

fn lookup_column(table_name: &str, column_name: &str) -> Result<Option<ColumnInfo>, spi::Error> {
    Spi::connect(|client| {
        let tup_table = client.select(
            "SELECT format_type(a.atttypid, a.atttypmod) AS type_name,
                            a.attnotnull AS not_null
                     FROM pg_catalog.pg_attribute a
                     WHERE a.attrelid = $1::regclass
                       AND a.attname = $2
                       AND a.attnum > 0
                       AND NOT a.attisdropped",
            None,
            &[table_name.into(), column_name.into()],
        )?;

        let row = tup_table.first();
        let type_name: Option<String> = row.get_by_name("type_name")?;
        let is_not_null: Option<bool> = row.get_by_name("not_null")?;

        Ok(type_name.map(|type_name| ColumnInfo {
            type_name,
            is_not_null: is_not_null.unwrap_or(false),
        }))
    })
}

fn sweep_delete(policy: &Policy) -> Result<(), spi::Error> {
    Spi::connect_mut(|client| {
        let query_text = client
            .select(
                "SELECT format(
                    'DELETE FROM %s WHERE %I < now() - interval %L',
                    $1::regclass, $2, $3
                 )",
                None,
                &[
                    policy.table_name.as_str().into(),
                    policy.age_column.as_str().into(),
                    format!("{} seconds", policy.max_age_seconds).into(),
                ],
            )?
            .first()
            .get_one::<String>()?
            .expect("format() should not return NULL");

        let result = client.update(&query_text, None, &[])?;
        let rows_affected = result.len() as i64;

        //TODO
        // log_sweep(client, policy, rows_affected)?;

        Ok(())
    })
}

fn is_timestamp_like(pg_type: &str) -> bool {
    matches!(
        pg_type,
        "timestamp without time zone" | "timestamp with time zone"
    )
}

fn is_text_like(pg_type: &str) -> bool {
    matches!(pg_type, "text" | "character varying" | "character" | "name")
        || pg_type.starts_with("character varying(")
        || pg_type.starts_with("character(")
}

fn run_sweep() {
    // fetch policies
    let q = format!(
        "SELECT id, table_name::text, age_column, max_age_seconds, mode::text,
                target_column, replacement_text
         FROM {}.policies
         WHERE is_enabled",
        NAME
    );

    let policies_result: Result<Vec<Policy>, spi::Error> = Spi::connect(|client| {
        client
            .select(&q, None, &[])?
            .map(|row| {
                Ok(Policy {
                    id: row["id"].value::<i32>()?.unwrap(),
                    table_name: row["table_name"].value::<String>()?.unwrap(),
                    age_column: row["age_column"].value::<String>()?.unwrap(),
                    max_age_seconds: row["max_age_seconds"].value::<i64>()?.unwrap(),
                    mode: row["mode"].value::<String>()?.unwrap(),
                    target_column: row["target_column"].value::<String>()?,
                    replacement_text: row["replacement_text"].value::<String>()?,
                })
            })
            .collect()
    });

    let policies = match policies_result {
        Ok(p) => p,
        Err(e) => {
            warning!("{NAME}: failed to read policies: {e}");
            return;
        }
    };

    for policy in &policies {
        let result = match policy.mode.as_str() {
            "delete_row" => sweep_delete(policy),
            "redact_column" => sweep_redact(policy),
            other => {
                warning!("{NAME}: unknown mode '{other}', skipping");
                continue;
            }
        };

        if let Err(e) = result {
            warning!(
                "{NAME}: sweep failed for {}.{}: {e}",
                policy.table_name,
                policy.age_column
            );
        }
    }
}

//todo: draft
fn log_sweep(
    client: &mut spi::SpiClient,
    policy: &Policy,
    rows_affected: i64,
) -> Result<(), spi::Error> {
    if rows_affected == 0 {
        return Ok(());
    }

    client.update(
        &format!(
            "INSERT INTO {NAME}.sweep_log
            (policy_id, table_name, age_column, mode, target_column, rows_affected)
         VALUES ($1, $2, $3, $4, $5, $6)"
        ),
        None,
        &[
            policy.id.into(),
            policy.table_name.as_str().into(),
            policy.age_column.as_str().into(),
            policy.mode.as_str().into(),
            policy.target_column.as_deref().into(),
            rows_affected.into(),
        ],
    )?;
    Ok(())
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_hello_pg_perishable() {
        assert_eq!("Hello, pg_perishable", crate::hello_pg_perishable());
    }
}

#[cfg(feature = "pg_bench")]
#[pg_schema]
mod benches {
    use pgrx::prelude::*;
    use pgrx_bench::{black_box, Bencher};

    #[pg_bench]
    fn bench_hello_pg_perishable(b: &mut Bencher) {
        b.iter(|| {
            black_box(crate::hello_pg_perishable());
        });
    }
}

/// This module is required by `cargo pgrx test` invocations.
/// It must be visible at the root of your extension crate.
#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {
        // perform one-off initialization when the pg_test framework starts
    }

    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        // return any postgresql.conf settings that are required for your tests
        vec![]
    }
}
