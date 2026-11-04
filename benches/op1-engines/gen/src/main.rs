//! Deterministic synthetic tables: `gen <rows> <dir>` writes facts_a.csv,
//! facts_b.csv (facts_a with 1% changed, 0.5% removed, 0.5% added) and dims.csv.
use std::{fs::File, io::{BufWriter, Write}};

const CITIES: [&str; 8] = ["Paris", "Lyon", "Lille", "Nantes", "Rennes", "Metz", "Brest", "Nice"];
const CODES: u64 = 50_000;

fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

fn row(w: &mut impl Write, id: u64, salt: u64) -> std::io::Result<()> {
    let h = mix(id);
    writeln!(
        w,
        "{id};CODE{};{:014};Organisation {:x};{};{}.{:02}",
        h % CODES,
        // ~10% distinct values: a high-cardinality GROUP BY that must spill.
        h % (id / 10 + 1),
        mix(h ^ salt),
        CITIES[(h >> 8) as usize % CITIES.len()],
        (h >> 16) % 100_000,
        (h >> 40) % 100
    )
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (rows, dir): (u64, &str) = (args[1].parse().unwrap(), &args[2]);
    const HEADER: &str = "id;code;siret;name;city;amount";
    let mut a = BufWriter::new(File::create(format!("{dir}/facts_a.csv"))?);
    let mut b = BufWriter::new(File::create(format!("{dir}/facts_b.csv"))?);
    writeln!(a, "{HEADER}")?;
    writeln!(b, "{HEADER}")?;
    for id in 0..rows {
        row(&mut a, id, 0)?;
        match id % 200 {
            0 => {}                                  // removed from b
            1 | 2 => row(&mut b, id, 1)?,            // changed in b
            _ => row(&mut b, id, 0)?,
        }
    }
    for id in rows..rows + rows / 200 {
        row(&mut b, id, 0)?; // added in b
    }
    let mut d = BufWriter::new(File::create(format!("{dir}/dims.csv"))?);
    writeln!(d, "code;family;active")?;
    for c in 0..CODES {
        writeln!(d, "CODE{c};F{};{}", c % 200, c % 3 != 0)?;
    }
    Ok(())
}
