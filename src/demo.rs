use cuckoo_hashing_relocation::{Config, CuckooMap, Event, Slot, Stop};
use std::{error::Error, fmt::Write as _, fs, path::Path};

type Snapshot = [Vec<Option<u64>>; 2];

fn snapshot(map: &CuckooMap) -> Snapshot {
    std::array::from_fn(|table| {
        (0..map.slots_per_table())
            .map(|index| map.key_at(Slot { table, index }))
            .collect()
    })
}

fn key_for(map: &CuckooMap, left: usize, right: usize, first: u64) -> u64 {
    (first..)
        .find(|&key| {
            let [a, b] = map.candidates(key);
            (a.index, b.index) == (left, right)
        })
        .expect("demo fixture should have a matching key")
}

pub fn write_examples(directory: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    for cycle in [false, true] {
        let (filename, svg) = example(cycle)?;
        fs::write(directory.join(filename), svg)?;
    }
    Ok(())
}

fn example(cycle: bool) -> Result<(&'static str, String), Box<dyn Error>> {
    let mut map = CuckooMap::new(Config {
        slots_per_table: 8,
        ..Config::default()
    });
    let (filename, existing, key, stop, caption) = if cycle {
        let a = key_for(&map, 0, 0, 0);
        let b = key_for(&map, 0, 0, a + 1);
        let c = key_for(&map, 0, 0, b + 1);
        (
            "cycle.svg",
            vec![a, b],
            c,
            Stop::Cycle,
            "The path revisits a slot. The rebuilt tables use fresh seeds.",
        )
    } else {
        let a = key_for(&map, 0, 0, 0);
        let c = key_for(&map, 1, 3, 0);
        let b = key_for(&map, 1, 0, 0);
        let d = key_for(&map, 0, 2, 0);
        let e = key_for(&map, 0, 2, d + 1);
        (
            "relocation.svg",
            vec![a, c, b, d],
            e,
            Stop::Empty,
            "Follow the arrows to the empty slot. The map applies the moves in reverse.",
        )
    };
    for key in existing {
        map.insert(key, key).map_err(|_| "demo setup failed")?;
    }
    let before = snapshot(&map);
    let mut events = Vec::new();
    map.insert_traced(key, key, |event| events.push(event))
        .map_err(|_| "demo insertion failed")?;
    let path = events
        .iter()
        .find_map(|event| match event {
            Event::Path {
                key: path_key,
                start,
                moves,
                stop: outcome,
            } if *path_key == key && *outcome == stop => Some((*start, moves)),
            _ => None,
        })
        .ok_or("expected a relocation path in the demo trace")?;
    let (start, moves) = path;
    let after = snapshot(&map);
    let height = 168 + 34 * before[0].len().max(after[0].len());
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="960" height="{height}" viewBox="0 0 960 {height}" role="img" aria-labelledby="title caption">
  <defs>
    <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="context-stroke"/></marker>
  </defs>
  <rect width="100%" height="100%" fill="#f4f7fb"/>
  <g font-family="system-ui, sans-serif" fill="#182638">
  <text id="title" x="32" y="40" font-size="25" font-weight="700">Cuckoo hashing · insert key {key}</text>
  <text id="caption" x="32" y="68" font-size="15">{caption}</text>
  <text x="32" y="109" font-size="17" font-weight="700">Before insertion</text>
  <text x="504" y="109" font-size="17" font-weight="700">After insertion</text>
"##
    );
    draw_tables(&mut svg, &before, 32, start, !cycle, key, false)?;
    draw_tables(&mut svg, &after, 504, start, !cycle, key, true)?;
    let color = if cycle { "#a53c27" } else { "#2453af" };
    let dashed = if cycle {
        r#" stroke-dasharray="6 4""#
    } else {
        ""
    };
    for (index, step) in moves.iter().enumerate() {
        let (x1, x2) = if step.from.table == 0 {
            (137, 352)
        } else {
            (352, 137)
        };
        let y1 = row_y(step.from.index) + 14;
        let y2 = row_y(step.to.index) + 14;
        let bend = if y1 == y2 {
            if step.from.table == 0 {
                -16
            } else {
                16
            }
        } else {
            0
        };
        let label_y = (y1 + y2) / 2 + bend - 5;
        let number = index + 1;
        writeln!(
            svg,
            r#"<path d="M{x1} {y1} Q245 {} {x2} {y2}" fill="none" stroke="{color}" stroke-width="2" marker-end="url(#arrow)"{dashed}/>"#,
            (y1 + y2) / 2 + 2 * bend
        )?;
        writeln!(
            svg,
            r#"<text x="245" y="{label_y}" text-anchor="middle" font-size="12" fill="{color}">{number}. key {}</text>"#,
            step.key
        )?;
    }
    svg.push_str("</g>\n</svg>\n");
    Ok((filename, svg))
}

fn row_y(index: usize) -> i32 {
    148 + index as i32 * 34
}

fn draw_tables(
    svg: &mut String,
    snapshot: &Snapshot,
    x: usize,
    start: Slot,
    direct_placement: bool,
    inserted_key: u64,
    after: bool,
) -> std::fmt::Result {
    for (table, buckets) in snapshot.iter().enumerate() {
        let x = x + table * 320;
        writeln!(
            svg,
            r#"<text x="{x}" y="137" font-size="14">Table {table}</text>"#
        )?;
        for (index, key) in buckets.iter().enumerate() {
            let y = row_y(index);
            let highlight = if after {
                *key == Some(inserted_key)
            } else {
                direct_placement && start == (Slot { table, index })
            };
            let fill = if highlight { "#d5e4ff" } else { "#ffffff" };
            let label = key.map_or_else(|| "·".to_owned(), |key| key.to_string());
            writeln!(
                svg,
                r##"<rect x="{x}" y="{y}" width="105" height="28" rx="4" fill="{fill}" stroke="#9cacc0"/><text x="{}" y="{}" font-size="12" fill="#55667c">{index}</text><text x="{}" y="{}" font-size="14" font-weight="600">{label}</text>"##,
                x + 9,
                y + 19,
                x + 35,
                y + 19
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagrams_show_committed_and_abandoned_paths() {
        let (_, relocation) = example(false).unwrap();
        assert_eq!(relocation.matches("marker-end=").count(), 3);
        assert!(!relocation.contains("stroke-dasharray"));
        let (_, cycle) = example(true).unwrap();
        assert_eq!(cycle.matches("marker-end=").count(), 2);
        assert!(cycle.contains("stroke-dasharray"));
        assert!(cycle.contains("fresh seeds"));
    }
}
