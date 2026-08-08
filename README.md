# pg_perishable

An extension for auto-removal of the table data.
The worker is **auto**-started; it survives:
 - restarts
 - crashes (provided that the Postgres process is rerstarted by the likes of supervisor)
 - redeploys (`shared_preload_libraries` must be preserved in your deployment config)
 - ~~and b0mb!ng of the data-center~~

One time set up is required; no manual `start_worker()` call is.


## Installation
- identify the location of the object code libraries
```
pg_config --sharedir
```
- copy the library, ***.so**, there
- specify it in the config

```
shared_preload_libraries = 'pg_perishable'
```


- restart Postgres; one-time
- `CREATE EXTENSION pg_perishable;`
