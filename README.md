# Cuckoo hashing relocation

A small, dependency-free Rust map for integer keys and values. Each key has one candidate bucket in each of two tables. Lookup and removal check at most two slots. An insertion can make room by moving resident keys to their alternate slots.

```sh
cargo test
cargo run -- uniform
cargo run -- clustered 5000 7
cargo run -- collision-heavy
cargo run -- demo
```

Workloads accept optional positional arguments for operation count, seed, initial slots per table, relocation limit, and target load percent. Defaults are 1,000 operations, seed 1, 16 slots per table, 32 moves per path, and 45% target load across both tables. The collision-heavy workload starts with three keys that share both initial candidate buckets and then switches to a mixed uniform workload.

The runner checks operations against Rust's standard `HashMap` and compares the full contents after rebuilds and at the end. It prints final load, bucket utilization, hit and miss probes, relocation lengths, rebuilds, and failed inserts. Bucket utilization counts live key/value bytes as a share of allocated bucket storage. Each workload writes a trace to `artifacts/<workload>.trace.txt`. Displacements between rebuild start and finish belong to the candidate tables. An unsuccessful rebuild discards those tables.

`cargo run -- demo` writes `artifacts/relocation.svg` and `artifacts/cycle.svg`. The first shows three moves that free a bucket at the original capacity. The second shows a path that revisits a bucket and the resulting layout after reseeding. Open either SVG in a browser.

## Insertion policy

A new entry uses an empty candidate directly when available and below the target load. Otherwise the map follows the alternate-bucket chain from each occupied candidate, stopping at an empty bucket, a repeated bucket, or the configured displacement limit. It plans each chain against the current layout and applies a successful plan from the end back to the start.

When neither chain succeeds, the map tries new seeds at the same capacity twice, then doubles the capacity for remaining attempts. Crossing the target load instead starts the rebuild at a larger capacity. A rebuild replaces the current tables only once every entry fits. Exhausting the rebuild budget leaves the map intact and returns an insertion error.
