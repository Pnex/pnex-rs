//! Pin self-test bench helper (D122) — used by `firmware/hil/pnex_hil.py`.
//!
//! ```text
//! cargo run -q -p pnex-core --example selftest -- plan <board>     # plan JSON
//! cargo run -q -p pnex-core --example selftest -- check <report>   # verdicts
//! ```
//!
//! `check` prints one line per step (`PASS`/`FAIL` + raw values) and exits
//! non-zero when the report does not satisfy the plan.

use std::process::ExitCode;

use pnex_core::caps::Soc;
use pnex_core::catalog;
use pnex_core::selftest::{check_report, evaluate, plan, Report};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["plan", board] => match catalog::board(board).and_then(plan) {
            Some(p) => {
                println!("{}", serde_json::to_string_pretty(&p).unwrap());
                ExitCode::SUCCESS
            }
            None => {
                let names: Vec<_> = catalog::boards()
                    .iter()
                    .filter(|b| b.soc.is_some())
                    .map(|b| b.name)
                    .collect();
                eprintln!("unknown MCU board {board}; known: {}", names.join(", "));
                ExitCode::from(2)
            }
        },
        ["check", path] => check(path),
        _ => {
            eprintln!("usage: selftest plan <board> | selftest check <report.json>");
            ExitCode::from(2)
        }
    }
}

fn check(path: &str) -> ExitCode {
    let report: Report = match std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(2);
        }
    };
    let Some(p) = catalog::board(&report.board).and_then(plan) else {
        eprintln!("unknown MCU board {}", report.board);
        return ExitCode::from(2);
    };
    let soc = Soc::from_board_soc(&p.soc).expect("catalog soc");
    for step in &p.steps {
        let raw = report
            .results
            .iter()
            .find(|r| r.gpio == step.gpio && r.mode == step.mode)
            .map(|r| &r.raw);
        let (verdict, detail) = match raw {
            None => ("MISS".to_string(), String::new()),
            Some(raw) => (
                match evaluate(soc, step, raw) {
                    Ok(()) => "PASS".to_string(),
                    Err(why) => format!("FAIL {why}"),
                },
                serde_json::to_string(raw).unwrap(),
            ),
        };
        println!(
            "{:<5} GPIO{:<3} {:<6} {:<18} {verdict:<24} {detail}",
            if verdict == "PASS" { "ok" } else { "!!" },
            step.gpio,
            step.label,
            step.mode.token(),
        );
    }
    for s in &p.skipped {
        println!("skip  GPIO{:<3} {:<6} {:?}", s.gpio, s.label, s.reason);
    }
    match check_report(&p, &report) {
        Ok(()) => {
            println!("{}: {} steps, all pass", p.board, p.steps.len());
            ExitCode::SUCCESS
        }
        Err(errs) => {
            for e in &errs {
                println!("ERROR {e}");
            }
            ExitCode::from(1)
        }
    }
}
