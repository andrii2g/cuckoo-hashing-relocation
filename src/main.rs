mod workload;

use cuckoo_hashing_relocation::Config;
use std::{error::Error, fs, io::BufWriter, path::Path};
use workload::Workload;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" {
        println!("Usage: cargo run -- <uniform|clustered|collision-heavy> [operations] [seed] [slots-per-table] [relocation-limit] [target-load-percent]");
        return Ok(());
    }
    if args.len() > 6 {
        return Err("too many arguments; use --help".into());
    }
    let workload = match args[0].as_str() {
        "uniform" => Workload::Uniform,
        "clustered" => Workload::Clustered,
        "collision-heavy" => Workload::CollisionHeavy,
        _ => return Err("unknown workload; use --help".into()),
    };
    let config = Config {
        seed: parse(&args, 2, 1)?,
        slots_per_table: parse(&args, 3, 16)?,
        relocation_limit: parse(&args, 4, 32)?,
        target_load_percent: parse(&args, 5, 45)?,
        ..Config::default()
    };
    if config.slots_per_table == 0 || !(1..=50).contains(&config.target_load_percent) {
        return Err(
            "slots-per-table must be positive and target-load-percent must be 1..=50".into(),
        );
    }
    let count = parse(&args, 1, 1_000)?;
    fs::create_dir_all("artifacts")?;
    let path = Path::new("artifacts").join(format!("{}.trace.txt", args[0]));
    let mut trace = BufWriter::new(fs::File::create(&path)?);
    let metrics = workload::run(workload, count, config, &mut trace)?;
    println!("{metrics}");
    println!("trace: {}", path.display());
    Ok(())
}

fn parse<T: std::str::FromStr>(
    args: &[String],
    index: usize,
    default: T,
) -> Result<T, Box<dyn Error>> {
    match args.get(index) {
        Some(value) => value
            .parse()
            .map_err(|_| format!("invalid numeric argument: {value}").into()),
        None => Ok(default),
    }
}
