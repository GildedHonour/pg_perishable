use pgrx::bgworkers::*;
use pgrx::prelude::*;
use std::time::Duration;

const NAME: &'static str = "pg_perishable";

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

const PG_ID_MAX_LEN: usize = 63;

fn ensure_age_index(client: &mut spi::SpiClient, table_name: &str, age_column: &str) -> Result<(), spi::Error> {
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
fn pg_perishable_create_policy(
    table_name: &str,
    age_column: &str,
    max_age_seconds: i64,
    is_soft_delete_enabled: default!(bool, false),
) -> Result<i64, spi::Error> {
    Spi::connect_mut(|client| {
        // a row for per policy
        // casting table_name::regclass ==> validation has been shifted to Postgres itself
        let row = client.update(
            "INSERT INTO pg_perishable.policies (table_name, age_column, max_age_seconds, is_soft_delete_enabled)
             VALUES ($1::regclass, $2, $3, $4)
             RETURNING id",
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
