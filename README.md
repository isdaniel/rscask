# RsCask

A log-structured key-value store written in Rust, implementing the [Bitcask](https://riak.com/assets/bitcask-intro.pdf) design. This workspace provides a core library, a Unix-socket daemon, and a CLI client.

## Crates

- `rscask`: library crate with the Bitcask implementation
- `rscaskd`: daemon that serves requests over a Unix socket
- `rscask-cli`: client for interacting with the daemon

## Library usage

```rust
use rscask::Bitcask;

fn main() -> rscask::Result<()> {
    let mut db = Bitcask::open("./db")?;
    
    // Write operations
    db.put("alpha", "one")?;
    db.put("beta", "two")?;
    
    // Read operations
    let value = db.get("alpha")?;
    println!("{value:?}");
    
    // Check existence
    if db.contains_key("beta") {
        println!("Key exists");
    }
    
    // Delete
    db.delete("beta")?;
    
    // Enumerate keys
    for key in db.keys() {
        println!("Key: {:?}", String::from_utf8_lossy(&key));
    }
    
    // Get statistics
    let stats = db.stats();
    println!("Keys in store: {}", stats.num_keys);
    
    // Compact to reclaim space
    db.merge()?;
    
    Ok(())
}
```

### Hint Files

After calling `merge()`, RsCask generates `.hint` files alongside compacted data segments. These contain only key metadata (no values), dramatically speeding up recovery:

- **Without hint files**: Recovery scans every value byte — O(total data size)
- **With hint files**: Recovery reads only key metadata — O(key count + active segment)

On restart, segments with hint files are loaded instantly; only the active segment requires a full scan.

## Daemon

Start the daemon:

```sh
cargo run -p rscaskd -- --db ./db
```

## CLI

Send commands to the daemon:

```sh
cargo run -p rscask-cli -- insert key value
cargo run -p rscask-cli -- update key value
cargo run -p rscask-cli -- get key
cargo run -p rscask-cli -- delete key
cargo run -p rscask-cli -- stats
cargo run -p rscask-cli -- merge
cargo run -p rscask-cli -- shutdown
```

For insert/update, pass `-` as the value to read from stdin.

## Architecture

### Record Format

Data files store records with CRC-32 integrity checking:

```
[crc:4][timestamp:8][flags:1][key_size:4][value_size:4][key:variable][value:variable]
```

Hint files store compact key metadata:

```
[crc:4][timestamp:8][key_size:4][file_id:8][offset:8][len:4][key:variable]
```

## Performance

Run benchmarks:

```sh
cargo bench -p rscask --bench bitcask_bench
```

Expected characteristics:
- **Writes**: Sequential append-only I/O approaching disk bandwidth
- **Reads**: O(1) memory lookup + 1 disk seek
- **Recovery**: Fast with hint files (O(key count) instead of O(data size))