//
// PneX pin self-test console (D122, docs/architecture/pin-selftest.md).
//
// Offline build (no WiFi, no server, no device config) driven over the
// serial port, 115200 baud, one command per line:
//
//   info                                  SoC, board, fw, ADC range
//   mode    <gpio> <mode> [safe_high]     configure (wire mode names, plus
//                                         digital_in_pullup)
//   write   <gpio> 0|1                    drive, answer with the pad level
//   pwm     <gpio> <duty 0..100>          drive, answer with the measured duty
//   read    <gpio>                        current value per the pin mode
//   release <gpio>                        stop PWM, back to floating input
//   test    <gpio> <mode> [safe_high]     full stimulus sequence of one
//                                         pin × mode, raw values only
//
// Every answer is ONE line `PNEXT {json}`; the host harness
// (firmware/hil/pnex_hil.py) ignores every other line. The pass/fail rules
// live in Rust (`pnex_core::selftest`), never here: the firmware reports
// what the pad does.
//
#ifndef PNEX_SELFTEST_H
#define PNEX_SELFTEST_H

/// Runs the console forever (call from setup(); never returns).
void pnex_selftest_run();

#endif  // PNEX_SELFTEST_H
