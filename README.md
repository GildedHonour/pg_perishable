# pg_perishable

An extension for auto-removal of the table data.
The worker is **auto**-started; it survives:
 - restarts
 - crashes (provided that the Postgres process is rerstarted by the likes of supervisor)
 - redeploys (`shared_preload_libraries` must be preserved in your deployment config)
 - ~~and b0mb!ng of the data-center~~

One time set up is required; no manual `start_worker()` call is.


## Installation

- add the library, ***.so**, into the config of Postgres

```
shared_preload_libraries = 'pg_perishable'
```


- restart Postgres; one-time
- `CREATE EXTENSION pg_perishable;`
