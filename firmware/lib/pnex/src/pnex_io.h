//
// PneX pin I/O primitives — the single place where a pin is configured,
// written and read. Shared by the WS pin_slave path (pnex.cpp) and the
// self-test console (pnex_selftest.cpp, D122): the self-test exercises the
// production code, never a copy.
//
// "Real" values: every read goes to the pad. On ESP32 the Arduino OUTPUT
// mode keeps the input buffer enabled, so the level read back after a
// write is the electrical level, not the output latch.
//
#ifndef PNEX_IO_H
#define PNEX_IO_H

#include <Arduino.h>

#include "Pnex.h"

// Upper bound of analogRead() on this SoC (10-bit ESP8266, 12-bit ESP32).
#if defined(ESP8266)
#define PNEX_ADC_MAX 1023
#else
#define PNEX_ADC_MAX 4095
#endif

/// pinMode + initial level (safe state) of a pin. False when a PWM pin
/// gets no hardware channel (ESP32 family: all LEDC channels in use).
bool pnex_io_apply(PnexPin& p);

/// Drives a digital output, then returns the level read back on the pad
/// (0/1). The caller compares it to the command.
int pnex_io_write_digital(uint8_t gpio, bool high);

/// Programs a PWM duty (0..=100 %, 8-bit Arduino scale) and records it.
void pnex_io_write_pwm(PnexPin& p, int duty_pct);

/// Current value of a pin per its mode: ADC raw count, PWM duty %, or the
/// digital pad level (0/1).
int pnex_io_read(const PnexPin& p);

/// Digital level on the pad, whatever the configured mode.
int pnex_io_level(uint8_t gpio);

/// Measured duty cycle (0..=100 %) of a running PWM output, by sampling the
/// pad with a jittered period over ~25 ms. Blocking: self-test only.
uint8_t pnex_io_measure_duty(uint8_t gpio);

/// Stops any PWM on the pin and leaves it as a floating input.
void pnex_io_release(uint8_t gpio);

/// Mode name as on the wire (`digital_out`, `adc_in`…).
const char* pnex_io_mode_name(uint8_t mode);

#endif  // PNEX_IO_H
