use std::mem::size_of;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub table: usize,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub key: u64,
    pub from: Slot,
    pub to: Slot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Empty,
    Cycle,
    Limit,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Path {
        key: u64,
        start: Slot,
        moves: Vec<Move>,
        stop: Stop,
    },
    Displacement(Move),
    RebuildStarted {
        slots_per_table: usize,
        seeds: [u64; 2],
    },
    RebuildFinished {
        installed: bool,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub slots_per_table: usize,
    pub seed: u64,
    pub relocation_limit: usize,
    pub target_load_percent: u8,
    pub rebuild_attempts: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            slots_per_table: 16,
            seed: 1,
            relocation_limit: 32,
            target_load_percent: 45,
            rebuild_attempts: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsertError;

#[derive(Clone, Copy)]
struct Entry {
    key: u64,
    value: u64,
}

struct Tables {
    buckets: [Vec<Option<Entry>>; 2],
    seeds: [u64; 2],
}

pub struct CuckooMap {
    tables: Tables,
    config: Config,
    seed_state: u64,
    len: usize,
}

// SplitMix64 finalizer. Wrapping arithmetic makes results independent of build mode.
pub fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

pub fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    mix64(*state)
}

impl Tables {
    fn new(size: usize, seed_state: &mut u64) -> Self {
        Self {
            buckets: [vec![None; size], vec![None; size]],
            seeds: [next_u64(seed_state), next_u64(seed_state)],
        }
    }

    fn candidate(&self, key: u64, table: usize) -> Slot {
        Slot {
            table,
            index: (mix64(key ^ self.seeds[table]) % self.buckets[table].len() as u64) as usize,
        }
    }

    fn at(&self, slot: Slot) -> Option<Entry> {
        self.buckets[slot.table][slot.index]
    }

    fn set(&mut self, slot: Slot, entry: Option<Entry>) {
        self.buckets[slot.table][slot.index] = entry;
    }

    fn place(&mut self, entry: Entry, limit: usize, trace: &mut impl FnMut(Event)) -> bool {
        let starts = [self.candidate(entry.key, 0), self.candidate(entry.key, 1)];
        for start in starts {
            if self.at(start).is_none() {
                self.set(start, Some(entry));
                return true;
            }
        }

        for start in starts {
            let mut slot = start;
            let mut moves = Vec::<Move>::new();
            let stop = loop {
                let Some(resident) = self.at(slot) else {
                    break Stop::Empty;
                };
                if moves.iter().any(|step| step.from == slot) {
                    break Stop::Cycle;
                }
                if moves.len() == limit {
                    break Stop::Limit;
                }
                let to = self.candidate(resident.key, 1 - slot.table);
                moves.push(Move {
                    key: resident.key,
                    from: slot,
                    to,
                });
                slot = to;
            };
            if stop == Stop::Empty {
                for step in moves.iter().rev() {
                    self.set(step.to, self.at(step.from));
                    trace(Event::Displacement(*step));
                }
                self.set(start, Some(entry));
            }
            trace(Event::Path {
                key: entry.key,
                start,
                moves,
                stop,
            });
            if stop == Stop::Empty {
                return true;
            }
        }
        false
    }
}

impl CuckooMap {
    pub fn new(config: Config) -> Self {
        assert!(config.slots_per_table > 0);
        assert!((1..=50).contains(&config.target_load_percent));
        let mut seed_state = config.seed;
        Self {
            tables: Tables::new(config.slots_per_table, &mut seed_state),
            config,
            seed_state,
            len: 0,
        }
    }

    pub fn candidates(&self, key: u64) -> [Slot; 2] {
        [self.tables.candidate(key, 0), self.tables.candidate(key, 1)]
    }

    pub fn key_at(&self, slot: Slot) -> Option<u64> {
        self.tables
            .buckets
            .get(slot.table)?
            .get(slot.index)?
            .map(|entry| entry.key)
    }

    fn find(&self, key: u64) -> (Option<Slot>, usize) {
        for (index, slot) in self.candidates(key).into_iter().enumerate() {
            if self.tables.at(slot).is_some_and(|entry| entry.key == key) {
                return (Some(slot), index + 1);
            }
        }
        (None, 2)
    }

    pub fn get_with_probes(&self, key: u64) -> (Option<u64>, usize) {
        let (slot, probes) = self.find(key);
        (
            slot.and_then(|slot| self.tables.at(slot))
                .map(|entry| entry.value),
            probes,
        )
    }

    pub fn get(&self, key: u64) -> Option<u64> {
        self.get_with_probes(key).0
    }

    pub fn remove(&mut self, key: u64) -> Option<u64> {
        let slot = self.find(key).0?;
        let value = self.tables.at(slot)?.value;
        self.tables.set(slot, None);
        self.len -= 1;
        Some(value)
    }

    pub fn insert(&mut self, key: u64, value: u64) -> Result<Option<u64>, InsertError> {
        self.insert_traced(key, value, |_| {})
    }

    pub fn insert_traced(
        &mut self,
        key: u64,
        value: u64,
        mut trace: impl FnMut(Event),
    ) -> Result<Option<u64>, InsertError> {
        let entry = Entry { key, value };
        if let Some(slot) = self.find(key).0 {
            let old = self.tables.at(slot).map(|entry| entry.value);
            self.tables.set(slot, Some(entry));
            return Ok(old);
        }

        let next_len = self.len as u128 + 1;
        let target = u128::from(self.config.target_load_percent);
        let over_target = |size: usize| next_len * 100 > size as u128 * 2 * target;
        let mut size = self.slots_per_table();
        let limit = self.config.relocation_limit;
        if !over_target(size) && self.tables.place(entry, limit, &mut trace) {
            self.len += 1;
            return Ok(None);
        }

        let mut seed_state = self.seed_state;
        for attempt in 0..self.config.rebuild_attempts {
            if attempt == 2 {
                size = size.checked_mul(2).ok_or(InsertError)?;
            }
            while over_target(size) {
                size = size.checked_mul(2).ok_or(InsertError)?;
            }
            let mut candidate = Tables::new(size, &mut seed_state);
            trace(Event::RebuildStarted {
                slots_per_table: size,
                seeds: candidate.seeds,
            });
            let installed = self
                .tables
                .buckets
                .iter()
                .flatten()
                .copied()
                .flatten()
                .chain(std::iter::once(entry))
                .all(|entry| candidate.place(entry, limit, &mut trace));
            if installed {
                self.tables = candidate;
                self.seed_state = seed_state;
                self.len += 1;
            }
            trace(Event::RebuildFinished { installed });
            if installed {
                return Ok(None);
            }
        }
        Err(InsertError)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn slots_per_table(&self) -> usize {
        self.tables.buckets[0].len()
    }

    pub fn bucket_bytes(&self) -> usize {
        self.tables
            .buckets
            .iter()
            .map(|table| table.capacity() * size_of::<Option<Entry>>())
            .sum()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.tables
            .buckets
            .iter()
            .flatten()
            .flatten()
            .map(|entry| (entry.key, entry.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relocates_multiple_keys_without_growing() {
        let mut map = CuckooMap::new(Config {
            slots_per_table: 8,
            ..Config::default()
        });
        let key = |left, right| {
            (0..)
                .find(|&key| {
                    let slots = map.candidates(key);
                    (slots[0].index, slots[1].index) == (left, right)
                })
                .unwrap()
        };
        let (a, b, c, d) = (key(0, 0), key(1, 0), key(1, 3), key(0, 2));
        // Fill d's right candidate with a key whose left candidate is already occupied.
        let e = (d + 1..)
            .find(|&key| map.candidates(key) == map.candidates(d))
            .unwrap();
        for key in [a, c, b, d] {
            map.insert(key, key).unwrap();
        }
        let mut events = Vec::new();
        map.insert_traced(e, e, |event| events.push(event)).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Displacement(_)))
                .count(),
            3
        );
        assert_eq!(map.slots_per_table(), 8);
        for key in [a, b, c, d, e] {
            assert_eq!(map.get(key), Some(key));
        }
    }

    #[test]
    fn cycle_rebuilds_or_preserves_the_map_on_failure() {
        for rebuild_attempts in [0, 8] {
            let mut map = CuckooMap::new(Config {
                rebuild_attempts,
                ..Config::default()
            });
            let pair = map.candidates(0);
            let keys: Vec<_> = (0..)
                .filter(|&key| map.candidates(key) == pair)
                .take(3)
                .collect();
            for &key in &keys[..2] {
                map.insert(key, key).unwrap();
            }
            let mut events = Vec::new();
            let result = map.insert_traced(keys[2], keys[2], |event| events.push(event));
            assert!(events.iter().any(|event| matches!(
                event,
                Event::Path {
                    stop: Stop::Cycle,
                    ..
                }
            )));
            assert_eq!(result.is_ok(), rebuild_attempts > 0);
            assert_eq!(map.len(), if result.is_ok() { 3 } else { 2 });
            for &key in &keys[..map.len()] {
                assert_eq!(map.get(key), Some(key));
            }
            assert_eq!(map.get(keys[2]), result.ok().map(|_| keys[2]));
        }
    }
}
