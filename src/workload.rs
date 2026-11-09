use cuckoo_hashing_relocation::{next_u64, Config, CuckooMap, Event, Stop};
use std::{
    collections::HashMap,
    fmt,
    io::{self, Write},
};

#[derive(Clone, Copy, Debug)]
pub enum Workload {
    Uniform,
    Clustered,
    CollisionHeavy,
}

#[derive(Default)]
struct Samples {
    count: usize,
    total: usize,
    max: usize,
}

impl Samples {
    fn record(&mut self, value: usize) {
        self.count += 1;
        self.total += value;
        self.max = self.max.max(value);
    }
}

impl fmt::Display for Samples {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.count == 0 {
            return f.write_str("no samples");
        }
        write!(
            f,
            "n={}, mean={:.2}, max={}",
            self.count,
            self.total as f64 / self.count as f64,
            self.max
        )
    }
}

#[derive(Default)]
pub struct Metrics {
    entries: usize,
    slots: usize,
    bucket_bytes: usize,
    hits: Samples,
    misses: Samples,
    applied_paths: Samples,
    abandoned_paths: Samples,
    rebuild_paths: Samples,
    displacements: usize,
    cycles: usize,
    limits: usize,
    rebuild_attempts: usize,
    rebuilds: usize,
    failed_inserts: usize,
    in_rebuild: bool,
}

impl Metrics {
    fn observe(&mut self, event: &Event) {
        match event {
            Event::Path { moves, stop, .. } => {
                match stop {
                    Stop::Cycle => self.cycles += 1,
                    Stop::Limit => self.limits += 1,
                    Stop::Empty => {}
                }
                let samples = if self.in_rebuild {
                    &mut self.rebuild_paths
                } else if *stop == Stop::Empty {
                    &mut self.applied_paths
                } else {
                    &mut self.abandoned_paths
                };
                samples.record(moves.len());
            }
            Event::Displacement(_) => self.displacements += 1,
            Event::RebuildStarted { .. } => {
                self.rebuild_attempts += 1;
                self.in_rebuild = true;
            }
            Event::RebuildFinished { installed } => {
                self.rebuilds += usize::from(*installed);
                self.in_rebuild = false;
            }
        }
    }
}

impl fmt::Display for Metrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "load: {}/{} ({:.2}%)",
            self.entries,
            self.slots,
            self.entries as f64 * 100.0 / self.slots as f64
        )?;
        writeln!(
            f,
            "bucket memory: {} bytes, {:.2}% live payload",
            self.bucket_bytes,
            self.entries as f64 * 16.0 * 100.0 / self.bucket_bytes as f64
        )?;
        writeln!(f, "lookup probes on hits: {}", self.hits)?;
        writeln!(f, "lookup probes on misses: {}", self.misses)?;
        writeln!(f, "applied relocation paths: {}", self.applied_paths)?;
        writeln!(f, "abandoned relocation paths: {}", self.abandoned_paths)?;
        writeln!(
            f,
            "relocation paths during rebuilds: {}",
            self.rebuild_paths
        )?;
        writeln!(
            f,
            "displacements: {}, cycles: {}, relocation limits reached: {}",
            self.displacements, self.cycles, self.limits
        )?;
        write!(
            f,
            "rebuild attempts: {}, installed rebuilds: {}, failed inserts: {}",
            self.rebuild_attempts, self.rebuilds, self.failed_inserts
        )
    }
}

fn collision_keys(map: &CuckooMap) -> io::Result<Vec<u64>> {
    let mut pairs = HashMap::<_, Vec<_>>::new();
    for key in 0..100_000 {
        let [left, right] = map.candidates(key);
        let keys = pairs.entry((left.index, right.index)).or_default();
        keys.push(key);
        if keys.len() == 3 {
            return Ok(std::mem::take(keys));
        }
    }
    Err(io::Error::other(
        "could not find a collision triple; use fewer initial slots",
    ))
}

fn key_for(workload: Workload, random: u64) -> u64 {
    match workload {
        Workload::Uniform | Workload::CollisionHeavy => random,
        Workload::Clustered => (random >> 62) * 1_000_000 + (random & 255),
    }
}

pub fn run(
    workload: Workload,
    count: usize,
    config: Config,
    trace: &mut impl Write,
) -> io::Result<Metrics> {
    let mut map = CuckooMap::new(config);
    let mut reference = HashMap::new();
    let mut live_keys = Vec::new();
    let mut state = config.seed ^ 0x74d2_c6bf_4173_f39a;
    let mut metrics = Metrics::default();
    let collisions = match workload {
        Workload::CollisionHeavy if count > 0 => collision_keys(&map)?,
        _ => Vec::new(),
    };
    writeln!(trace, "{workload:?} operations={count} {config:?}")?;

    for operation in 0..count {
        let random = next_u64(&mut state);
        let fresh_key = key_for(workload, random);
        let existing_index = (!live_keys.is_empty()).then(|| random as usize % live_keys.len());
        let choice = next_u64(&mut state) % 10;

        if operation < collisions.len() || choice < 5 {
            let key = collisions.get(operation).copied().unwrap_or_else(|| {
                if choice == 0 {
                    existing_index.map_or(fresh_key, |index| live_keys[index])
                } else {
                    fresh_key
                }
            });
            let value = next_u64(&mut state);
            writeln!(trace, "{operation} insert {key} {value}")?;
            let mut trace_result = Ok(());
            let attempts_before = metrics.rebuild_attempts;
            match map.insert_traced(key, value, |event| {
                metrics.observe(&event);
                if trace_result.is_ok() {
                    trace_result = writeln!(trace, "  {event:?}");
                }
            }) {
                Ok(old) => {
                    assert_eq!(
                        old,
                        reference.insert(key, value),
                        "update at operation {operation}"
                    );
                    if old.is_none() {
                        live_keys.push(key);
                    }
                }
                Err(_) => {
                    assert!(!reference.contains_key(&key));
                    metrics.failed_inserts += 1;
                    writeln!(trace, "  InsertFailed")?;
                    verify_contents(&map, &reference);
                }
            }
            trace_result?;
            if metrics.rebuild_attempts != attempts_before {
                verify_contents(&map, &reference);
            }
        } else if choice == 9 {
            if let Some(index) = existing_index {
                let key = live_keys.swap_remove(index);
                assert_eq!(
                    map.remove(key),
                    reference.remove(&key),
                    "delete at operation {operation}"
                );
            } else {
                assert_eq!(map.remove(fresh_key), reference.remove(&fresh_key));
            }
        } else {
            let key = if choice < 7 {
                existing_index.map_or(fresh_key, |index| live_keys[index])
            } else {
                fresh_key
            };
            let (actual, probes) = map.get_with_probes(key);
            assert_eq!(
                actual,
                reference.get(&key).copied(),
                "lookup at operation {operation}"
            );
            if actual.is_some() {
                metrics.hits.record(probes);
            } else {
                metrics.misses.record(probes);
            }
        }
        assert_eq!(
            map.len(),
            reference.len(),
            "length at operation {operation}"
        );
    }

    verify_contents(&map, &reference);
    trace.flush()?;
    metrics.entries = map.len();
    metrics.slots = map.slots_per_table() * 2;
    metrics.bucket_bytes = map.bucket_bytes();
    Ok(metrics)
}

fn verify_contents(map: &CuckooMap, reference: &HashMap<u64, u64>) {
    assert_eq!(map.iter().collect::<HashMap<_, _>>(), *reference);
    for (&key, &value) in reference {
        assert_eq!(map.get(key), Some(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_workloads_match_reference_and_replay() {
        for workload in [
            Workload::Uniform,
            Workload::Clustered,
            Workload::CollisionHeavy,
        ] {
            for seed in [1, 2, 17] {
                let config = Config {
                    seed,
                    ..Config::default()
                };
                let mut first_trace = Vec::new();
                let first = run(workload, 2_000, config, &mut first_trace).unwrap();
                let mut second_trace = Vec::new();
                let second = run(workload, 2_000, config, &mut second_trace).unwrap();
                assert_eq!(first_trace, second_trace);
                assert_eq!(first.to_string(), second.to_string());
                assert_eq!(first.failed_inserts, 0);
                assert_eq!(first.hits.max, 2);
                assert_eq!(first.misses.max, 2);
                if matches!(workload, Workload::CollisionHeavy) {
                    assert!(first.cycles > 0);
                    assert!(first.rebuilds > 0);
                }
            }
        }
    }
}
