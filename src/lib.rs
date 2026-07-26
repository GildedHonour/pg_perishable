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
        id              SERIAL PRIMARY KEY,
        table_name      regclass    NOT NULL,   -- validated: must exist
        column_name     name        NOT NULL,   -- the timestamp column
        ttl_seconds     bigint      NOT NULL CHECK (ttl_seconds > 0),
        soft_delete     boolean     NOT NULL DEFAULT false,
        enabled         boolean     NOT NULL DEFAULT true,
        created_at      timestamptz NOT NULL DEFAULT now(),

        UNIQUE (table_name, column_name)
    );
    "#,
    name = "pg_perishable_policies_table",
);

#[pg_extern]
fn pg_perishable_create_policy(
    table_name: &str,
    column_name: &str,
    ttl_seconds: i64,
    soft_delete: default!(bool, false),
) -> Result<i64, spi::Error> {
    Spi::connect_mut(|client| {
        // a row for per policy
        // casting table_name::regclass ==> validation has been shifted to Postgres itself
        let row = client.update(
            "INSERT INTO pg_perishable.policies (table_name, column_name, ttl_seconds, soft_delete)
             VALUES ($1::regclass, $2, $3, $4)
             RETURNING id",
            None,
            &[
                table_name.into(),
                column_name.into(),
                ttl_seconds.into(),
                soft_delete.into(),
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
