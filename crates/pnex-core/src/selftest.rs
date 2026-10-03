//! Pin self-test bench (D122, `docs/architecture/pin-selftest.md`).
//!
//! Every catalog board ships with a hardware-in-the-loop report: the bench
//! firmware (`firmware/selftest`, console `PNEXT {json}`) exercises each
//! profile GPIO in each mode the chip caps allow and reports the RAW values
//! read on the pad; this module owns the rest:
//!
//! - [`plan`] — which `(gpio, mode)` steps a board must pass, derived from
//!   its profile and `caps` (never written by hand);
//! - [`evaluate`] — the pass/fail rule of one step, from the raw values
//!   (the firmware and the host harness never decide);
//! - [`check_report`] — a committed report covers exactly the plan and
//!   every step passes. Enforced in CI for every board not in
//!   [`HIL_PENDING`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::caps::{self, PinFlag, Soc};
use crate::catalog::CatalogBoard;
use crate::proto::{Mode, ModeOpts, SafeState};

/// Boards admitted before D122 and not yet bench-tested. This list may only
/// shrink: a new board never enters it, and a board leaves it with its
/// report (`firmware/hil/reports/<name>.json`).
pub const HIL_PENDING: &[&str] = &[
    "esp8266",
    "nodemcu_oled",
    "esp32-wroom-32",
    "esp32-devkit-v1-36p",
    "esp32-devkit-tft-st7735",
    "esp32-s3",
    "esp32-devkitc-v4-38p",
    "nodemcu_v3_oled",
];

/// Ratchet on [`HIL_PENDING`]: lower it each time a board leaves the list,
/// never raise it.
pub const HIL_PENDING_MAX: usize = 8;

/// Self-test mode: the wire modes plus the pull-up variant of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestMode {
    DigitalIn,
    DigitalInPullup,
    AdcIn,
    DigitalOut,
    PwmOut,
}

impl TestMode {
    /// Console token (`test <gpio> <mode>`).
    pub fn token(self) -> &'static str {
        match self {
            Self::DigitalIn => "digital_in",
            Self::DigitalInPullup => "digital_in_pullup",
            Self::AdcIn => "adc_in",
            Self::DigitalOut => "digital_out",
            Self::PwmOut => "pwm_out",
        }
    }
}

/// Why a profile GPIO has no step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// UART0 console: driving it kills the bench link.
    Console,
    /// Wired to a soldered screen.
    BuiltinScreen,
    /// No mode allowed by the chip caps.
    NoMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub gpio: u16,
    pub label: String,
    pub mode: TestMode,
    /// Outputs on a boot-HIGH strapping pin rest high.
    pub safe_high: bool,
    /// Board-wired pull-down (profile `pull_down`): the internal pull-up
    /// must read LOW — the bench confirms the declared wiring.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub board_pull_down: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSkip {
    pub gpio: u16,
    pub label: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub board: String,
    /// `mcu_boards.soc` convention (« esp8266 », « esp32-c3 »…).
    pub soc: String,
    /// PlatformIO board id (`PNEX_PIO_BOARD`).
    pub pio_board: String,
    /// `firmware/selftest` env.
    pub pio_env: String,
    pub adc_max: u32,
    pub steps: Vec<PlanStep>,
    pub skipped: Vec<PlanSkip>,
}

/// `firmware/selftest` env of a SoC.
pub fn pio_env(soc: Soc) -> &'static str {
    match soc {
        Soc::Esp8266 => "esp8266",
        Soc::Esp32 => "esp32",
        Soc::Esp32C3 => "esp32c3",
        Soc::Esp32S3 => "esp32s3",
    }
}

/// Full scale of `analogRead` (10-bit ESP8266, 12-bit ESP32 family).
pub fn adc_max(soc: Soc) -> u32 {
    match soc {
        Soc::Esp8266 => 1023,
        _ => 4095,
    }
}

/// Resting level of an output step: low when legal, else high (boot-HIGH
/// strapping); `None` when no safe state is legal.
fn output_safe_high(soc: Soc, gpio: u16, mode: Mode) -> Option<bool> {
    let opts = |s| ModeOpts {
        pullup: None,
        safe_state: Some(s),
    };
    if caps::validate(soc, gpio, mode, &opts(SafeState::Low)).is_ok() {
        Some(false)
    } else if caps::validate(soc, gpio, mode, &opts(SafeState::High)).is_ok() {
        Some(true)
    } else {
        None
    }
}

/// Steps of a board, in profile order; `None` for a non-MCU board.
pub fn plan(board: &CatalogBoard) -> Option<Plan> {
    let soc = board.soc?;
    let profile = (board.profile?)();
    let screen_gpios: BTreeSet<u16> = profile
        .peripherals
        .screens
        .iter()
        .filter(|s| s.builtin)
        .flat_map(|s| s.pins.values().copied())
        .collect();

    let mut steps = Vec::new();
    let mut skipped = Vec::new();
    for pin in &profile.pins {
        let Some(gpio) = pin.gpio else { continue };
        let skip = |reason| PlanSkip {
            gpio,
            label: pin.label.clone(),
            reason,
        };
        if caps::is_console_pin(soc, gpio) {
            skipped.push(skip(SkipReason::Console));
            continue;
        }
        if screen_gpios.contains(&gpio) {
            skipped.push(skip(SkipReason::BuiltinScreen));
            continue;
        }
        let before = steps.len();
        let mut step = |mode, safe_high| {
            steps.push(PlanStep {
                gpio,
                label: pin.label.clone(),
                mode,
                safe_high,
                board_pull_down: pin.pull_down,
            })
        };
        let modes = caps::available_modes(soc, gpio);
        if modes.contains(&Mode::DigitalIn) {
            step(TestMode::DigitalIn, false);
            let pullup = ModeOpts {
                pullup: Some(true),
                safe_state: None,
            };
            if caps::validate(soc, gpio, Mode::DigitalIn, &pullup).is_ok() {
                step(TestMode::DigitalInPullup, false);
            }
        }
        if modes.contains(&Mode::AdcIn) {
            step(TestMode::AdcIn, false);
        }
        for (mode, test) in [
            (Mode::DigitalOut, TestMode::DigitalOut),
            (Mode::PwmOut, TestMode::PwmOut),
        ] {
            if modes.contains(&mode) {
                if let Some(high) = output_safe_high(soc, gpio, mode) {
                    step(test, high);
                }
            }
        }
        if steps.len() == before {
            skipped.push(skip(SkipReason::NoMode));
        }
    }
    Some(Plan {
        board: board.name.into(),
        soc: soc.name().into(),
        pio_board: board.pio_board.unwrap_or_default().into(),
        pio_env: pio_env(soc).into(),
        adc_max: adc_max(soc),
        steps,
        skipped,
    })
}

/// Bench level: L1 = bare board (read-back on the pad), L2 = loopback jig.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    L1,
    L2,
}

/// Raw values of one step, as the firmware printed them (`PNEXT` test line).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRaw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<Vec<i64>>,
    /// Pad level read back after writing 0 / 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w0: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w1: Option<i64>,
    /// Commanded duty % → measured duty % on the pad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<BTreeMap<String, i64>>,
    /// Firmware refusal (`protected_pin`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepResult {
    pub gpio: u16,
    pub mode: TestMode,
    pub raw: StepRaw,
}

/// A committed bench report (`firmware/hil/reports/<board>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub board: String,
    pub soc: String,
    pub level: Level,
    /// Bench date (ISO 8601).
    pub date: String,
    /// `PNEXT info` of the flashed bench firmware.
    pub chip: String,
    pub results: Vec<StepResult>,
}

/// Pass/fail of one step from its raw values — `Err` = machine reason.
pub fn evaluate(soc: Soc, step: &PlanStep, raw: &StepRaw) -> Result<(), String> {
    if let Some(err) = &raw.err {
        return Err(format!("firmware_error:{err}"));
    }
    let samples = || match &raw.samples {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err("no_samples".to_string()),
    };
    match step.mode {
        TestMode::DigitalIn => {
            if samples()?.iter().all(|v| *v == 0 || *v == 1) {
                Ok(())
            } else {
                Err("not_a_logic_level".into())
            }
        }
        TestMode::DigitalInPullup => {
            let s = samples()?;
            // Boards wire external resistors on strapping pins (NodeMCU
            // GPIO15 pull-down): the internal pull-up loses, any level goes.
            let strapping = caps::pin_flags(soc, step.gpio).contains(&PinFlag::Strapping);
            if step.board_pull_down {
                return if s.iter().all(|v| *v == 0) {
                    Ok(())
                } else {
                    Err("board_pull_down_not_seen".into())
                };
            }
            if s.iter().all(|v| *v == 1) || (strapping && s.iter().all(|v| *v == 0 || *v == 1)) {
                Ok(())
            } else {
                Err("pullup_not_high".into())
            }
        }
        TestMode::AdcIn => {
            let max = i64::from(adc_max(soc));
            if samples()?.iter().all(|v| (0..=max).contains(v)) {
                Ok(())
            } else {
                Err("adc_out_of_range".into())
            }
        }
        TestMode::DigitalOut => match (raw.w0, raw.w1) {
            (Some(0), Some(1)) => Ok(()),
            (Some(_), Some(_)) => Err("readback_mismatch".into()),
            _ => Err("no_readback".into()),
        },
        TestMode::PwmOut => {
            let m = raw.measured.as_ref().ok_or("no_measure")?;
            let get = |d: &str| m.get(d).copied().ok_or(format!("no_measure:{d}"));
            let (d0, d50, d100) = (get("0")?, get("50")?, get("100")?);
            if d0 > 5 {
                Err("duty_0_not_low".into())
            } else if d100 < 95 {
                Err("duty_100_not_high".into())
            } else if !(35..=65).contains(&d50) {
                Err("duty_50_off".into())
            } else {
                Ok(())
            }
        }
    }
}

/// A report matches its plan exactly and every step passes; `Err` lists
/// every problem (one line each).
pub fn check_report(plan: &Plan, report: &Report) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if report.board != plan.board {
        errors.push(format!("board {} != {}", report.board, plan.board));
    }
    if report.soc != plan.soc {
        errors.push(format!("soc {} != {}", report.soc, plan.soc));
    }
    let Some(soc) = Soc::from_board_soc(&plan.soc) else {
        errors.push(format!("unknown soc {}", plan.soc));
        return Err(errors);
    };
    let mut results: BTreeMap<(u16, TestMode), &StepResult> = BTreeMap::new();
    for r in &report.results {
        if results.insert((r.gpio, r.mode), r).is_some() {
            errors.push(format!("GPIO{} {}: reported twice", r.gpio, r.mode.token()));
        }
    }
    for step in &plan.steps {
        match results.remove(&(step.gpio, step.mode)) {
            None => errors.push(format!(
                "GPIO{} ({}) {}: not tested",
                step.gpio,
                step.label,
                step.mode.token()
            )),
            Some(r) => {
                if let Err(why) = evaluate(soc, step, &r.raw) {
                    errors.push(format!(
                        "GPIO{} ({}) {}: {why}",
                        step.gpio,
                        step.label,
                        step.mode.token()
                    ));
                }
            }
        }
    }
    for (gpio, mode) in results.keys() {
        errors.push(format!("GPIO{gpio} {}: not in the plan", mode.token()));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::catalog;

    fn reports_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../firmware/hil/reports")
    }

    fn passing_raw(mode: TestMode) -> StepRaw {
        match mode {
            TestMode::DigitalIn | TestMode::DigitalInPullup => StepRaw {
                samples: Some(vec![1; 5]),
                ..Default::default()
            },
            TestMode::AdcIn => StepRaw {
                samples: Some(vec![12; 10]),
                ..Default::default()
            },
            TestMode::DigitalOut => StepRaw {
                w0: Some(0),
                w1: Some(1),
                ..Default::default()
            },
            TestMode::PwmOut => StepRaw {
                measured: Some(
                    [("0", 0), ("50", 50), ("100", 100)]
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                ),
                ..Default::default()
            },
        }
    }

    fn passing_report(plan: &Plan) -> Report {
        Report {
            board: plan.board.clone(),
            soc: plan.soc.clone(),
            level: Level::L1,
            date: "2026-10-03".into(),
            chip: plan.soc.clone(),
            results: plan
                .steps
                .iter()
                .map(|s| StepResult {
                    gpio: s.gpio,
                    mode: s.mode,
                    raw: passing_raw(s.mode),
                })
                .collect(),
        }
    }

    #[test]
    fn nodemcu_plan_follows_the_chip_caps() {
        let plan = plan(catalog::board("esp8266").unwrap()).unwrap();
        let modes = |g: u16| -> Vec<TestMode> {
            plan.steps
                .iter()
                .filter(|s| s.gpio == g)
                .map(|s| s.mode)
                .collect()
        };
        // A0: ADC only.
        assert_eq!(modes(17), vec![TestMode::AdcIn]);
        // GPIO16: no pull-up, no PWM.
        assert_eq!(modes(16), vec![TestMode::DigitalIn, TestMode::DigitalOut]);
        // Console pins are never driven.
        assert!(modes(1).is_empty() && modes(3).is_empty());
        assert!(plan
            .skipped
            .iter()
            .any(|s| s.gpio == 1 && s.reason == SkipReason::Console));
    }

    #[test]
    fn strapping_high_outputs_rest_high() {
        // XIAO C3: GPIO8/9 are boot-HIGH strapping pins.
        let plan = plan(catalog::board("esp32-c3").unwrap()).unwrap();
        for g in [8, 9] {
            assert!(plan
                .steps
                .iter()
                .any(|s| s.gpio == g && s.mode == TestMode::DigitalOut && s.safe_high));
        }
    }

    #[test]
    fn builtin_screen_pins_are_skipped() {
        let plan = plan(catalog::board("nodemcu_v3_oled").unwrap()).unwrap();
        for g in [12, 14] {
            assert!(plan.steps.iter().all(|s| s.gpio != g));
            assert!(plan
                .skipped
                .iter()
                .any(|s| s.gpio == g && s.reason == SkipReason::BuiltinScreen));
        }
    }

    #[test]
    fn check_report_rejects_gaps_failures_and_extras() {
        let plan = plan(catalog::board("esp8266").unwrap()).unwrap();
        let ok = passing_report(&plan);
        assert_eq!(check_report(&plan, &ok), Ok(()));

        let mut missing = ok.clone();
        missing.results.pop();
        assert!(check_report(&plan, &missing).unwrap_err()[0].contains("not tested"));

        let mut stuck = ok.clone();
        let out = stuck
            .results
            .iter_mut()
            .find(|r| r.mode == TestMode::DigitalOut)
            .unwrap();
        out.raw.w1 = Some(0);
        assert!(check_report(&plan, &stuck).unwrap_err()[0].contains("readback_mismatch"));

        let mut extra = ok.clone();
        extra.results.push(StepResult {
            gpio: 1,
            mode: TestMode::DigitalOut,
            raw: passing_raw(TestMode::DigitalOut),
        });
        assert!(check_report(&plan, &extra).unwrap_err()[0].contains("not in the plan"));
    }

    #[test]
    fn pwm_and_pullup_rules() {
        let soc = Soc::Esp8266;
        let step = |gpio, mode| PlanStep {
            gpio,
            label: "x".into(),
            mode,
            safe_high: false,
            board_pull_down: false,
        };
        let mut pwm = passing_raw(TestMode::PwmOut);
        pwm.measured.as_mut().unwrap().insert("50".into(), 80);
        assert_eq!(
            evaluate(soc, &step(5, TestMode::PwmOut), &pwm),
            Err("duty_50_off".into())
        );
        let low = StepRaw {
            samples: Some(vec![0; 5]),
            ..Default::default()
        };
        // GPIO15 (strapping, board pull-down): low is fine; GPIO5: it is not.
        assert_eq!(
            evaluate(soc, &step(15, TestMode::DigitalInPullup), &low),
            Ok(())
        );
        assert!(evaluate(soc, &step(5, TestMode::DigitalInPullup), &low).is_err());
    }

    /// D122 gate: every MCU board of the catalog carries a passing bench
    /// report, unless it predates D122 and is still listed in HIL_PENDING.
    #[test]
    fn every_board_has_a_passing_bench_report() {
        assert!(
            HIL_PENDING.len() <= HIL_PENDING_MAX,
            "HIL_PENDING only shrinks: a new board needs its bench report (pin-selftest.md)"
        );
        let mut problems = Vec::new();
        for name in HIL_PENDING {
            if catalog::board(name).is_none() {
                problems.push(format!("HIL_PENDING: unknown board {name}"));
            }
        }
        for board in catalog::boards() {
            let Some(plan) = plan(board) else { continue };
            let path = reports_dir().join(format!("{}.json", board.name));
            let pending = HIL_PENDING.contains(&board.name);
            let Ok(text) = std::fs::read_to_string(&path) else {
                if !pending {
                    problems.push(format!(
                        "{}: no bench report at {} — run firmware/hil/pnex_hil.py",
                        board.name,
                        path.display()
                    ));
                }
                continue;
            };
            let report: Report = match serde_json::from_str(&text) {
                Ok(r) => r,
                Err(e) => {
                    problems.push(format!("{}: unreadable report: {e}", board.name));
                    continue;
                }
            };
            match check_report(&plan, &report) {
                Ok(()) if pending => problems.push(format!(
                    "{}: report passes — remove it from HIL_PENDING and lower HIL_PENDING_MAX",
                    board.name
                )),
                Ok(()) => {}
                Err(errs) => {
                    for e in errs {
                        problems.push(format!("{}: {e}", board.name));
                    }
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }
}
