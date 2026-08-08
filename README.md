# pg_perishable

An extension for auto-removal of the table data.
The worker is **auto**-started; it survives:
 - restarts
 - crashes (provided that the Postgres process is rerstarted by the likes of supervisor)
 - redeploys (`shared_preload_libraries` must be preserved in your deployment config)
 - ~~and b0mb!ng of the data-center~~

One time set up is required; no manual `start_worker()` call is.


## Installation

download a release:
```
curl -LO {...}/releases/download/{ver}/{rels_name}.tar.gz
```

unpack it:
```
tar -xzf {rels_name}.tar.gz
cd {rels_name}
```

install it:
```
sudo make install
```

Finish by following the post-installation instructions that'll be printed.
