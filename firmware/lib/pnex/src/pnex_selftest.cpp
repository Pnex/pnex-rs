//
// PneX pin self-test console — see pnex_selftest.h.
//

#include "pnex_selftest.h"

#include <ArduinoJson.h>

#include "Pnex.h"
#include "pnex_io.h"

// Production builds must never carry the console: the server-built generic
// projects define PNEX_OTA_ENABLE=1, the self-test project never does.
#if defined(PNEX_TEST_MODE) && PNEX_TEST_MODE == 1 && PNEX_OTA_ENABLE == 1
#error "PNEX_TEST_MODE is a bench build: it cannot be combined with a server-built firmware"
#endif

namespace {

// Self-test mode set: the wire modes plus the pull-up variant of the input.
enum TestMode : uint8_t {
    TM_DIGITAL_IN,
    TM_DIGITAL_IN_PULLUP,
    TM_DIGITAL_OUT,
    TM_PWM_OUT,
    TM_ADC_IN,
    TM_UNKNOWN,
};

const uint8_t DIGITAL_SAMPLES = 5;
const uint8_t ADC_SAMPLES = 10;

TestMode parse_mode(const char* s) {
    if (strcmp(s, "digital_in") == 0) return TM_DIGITAL_IN;
    if (strcmp(s, "digital_in_pullup") == 0) return TM_DIGITAL_IN_PULLUP;
    if (strcmp(s, "digital_out") == 0) return TM_DIGITAL_OUT;
    if (strcmp(s, "pwm_out") == 0) return TM_PWM_OUT;
    if (strcmp(s, "adc_in") == 0 || strcmp(s, "analog_in") == 0) return TM_ADC_IN;
    return TM_UNKNOWN;
}

PnexPin make_pin(uint8_t gpio, TestMode m, bool safe_high) {
    PnexPin p{};
    p.gpio = gpio;
    p.safe_high = safe_high;
    p.pullup = m == TM_DIGITAL_IN_PULLUP;
    switch (m) {
        case TM_DIGITAL_OUT:
            p.mode = PNEX_DIGITAL_OUT;
            break;
        case TM_PWM_OUT:
            p.mode = PNEX_PWM_OUT;
            break;
        case TM_ADC_IN:
            p.mode = PNEX_ADC_IN;
            break;
        default:
            p.mode = PNEX_DIGITAL_IN;
            break;
    }
    return p;
}

// Pins whose use would kill the running test itself: SPI flash/PSRAM and
// the serial console (UART0 or native USB). Everything else is the
// server's job (chip caps).
bool is_protected(uint8_t gpio) {
#if defined(ESP8266)
    return (gpio >= 6 && gpio <= 11) || gpio == 1 || gpio == 3;
#elif defined(CONFIG_IDF_TARGET_ESP32C3)
    return (gpio >= 11 && gpio <= 17) || gpio == 18 || gpio == 19;
#elif defined(CONFIG_IDF_TARGET_ESP32S3)
    return (gpio >= 26 && gpio <= 37) || gpio == 19 || gpio == 20 || gpio == 43 || gpio == 44;
#else
    return (gpio >= 6 && gpio <= 11) || gpio == 1 || gpio == 3;
#endif
}

// Pins configured with `mode` (ad hoc console use).
PnexPin s_pins[PNEX_MAX_PINS];
uint8_t s_pin_count = 0;

PnexPin* find_pin(uint8_t gpio) {
    for (uint8_t i = 0; i < s_pin_count; ++i) {
        if (s_pins[i].gpio == gpio) {
            return &s_pins[i];
        }
    }
    return nullptr;
}

PnexPin* upsert_pin(const PnexPin& p) {
    PnexPin* slot = find_pin(p.gpio);
    if (!slot) {
        if (s_pin_count >= PNEX_MAX_PINS) {
            return nullptr;
        }
        slot = &s_pins[s_pin_count++];
    }
    *slot = p;
    return slot;
}

void forget_pin(uint8_t gpio) {
    for (uint8_t i = 0; i < s_pin_count; ++i) {
        if (s_pins[i].gpio == gpio) {
            s_pins[i] = s_pins[--s_pin_count];
            return;
        }
    }
}

void emit(const JsonDocument& doc) {
    Serial.print("PNEXT ");
    serializeJson(doc, Serial);
    Serial.println();
}

void emit_error(const char* op, const char* err) {
    JsonDocument doc;
    doc["op"] = op;
    doc["err"] = err;
    emit(doc);
}

void emit_info(const char* op) {
    JsonDocument doc;
    doc["op"] = op;
    doc["chip"] = PNEX_CHIP;
    doc["board"] = PNEX_BOARD_NAME;
    doc["fw"] = PNEX_FW_VERSION;
    doc["adc_max"] = PNEX_ADC_MAX;
    emit(doc);
}

int adc_sample(const PnexPin& p) {
    delay(2);
    return pnex_io_read(p);
}

// One pin × mode, raw values only (the verdict is computed host-side).
void run_test(uint8_t gpio, TestMode m, bool safe_high, const char* mode_name) {
    PnexPin p = make_pin(gpio, m, safe_high);
    JsonDocument doc;
    doc["op"] = "test";
    doc["gpio"] = gpio;
    doc["mode"] = mode_name;
    pnex_io_apply(p);
    switch (m) {
        case TM_DIGITAL_IN:
        case TM_DIGITAL_IN_PULLUP: {
            delay(5);  // let the pull-up charge the pad
            JsonArray a = doc["samples"].to<JsonArray>();
            for (uint8_t i = 0; i < DIGITAL_SAMPLES; ++i) {
                a.add(pnex_io_level(gpio));
                delay(2);
            }
            break;
        }
        case TM_ADC_IN: {
            JsonArray a = doc["samples"].to<JsonArray>();
            for (uint8_t i = 0; i < ADC_SAMPLES; ++i) {
                a.add(adc_sample(p));
            }
            break;
        }
        case TM_DIGITAL_OUT:
            doc["w0"] = pnex_io_write_digital(gpio, false);
            doc["w1"] = pnex_io_write_digital(gpio, true);
            pnex_io_write_digital(gpio, safe_high);
            break;
        case TM_PWM_OUT: {
            const uint8_t duties[] = {0, 50, 100};
            JsonObject measured = doc["measured"].to<JsonObject>();
            for (uint8_t d : duties) {
                pnex_io_write_pwm(p, d);
                delay(5);
                measured[String(d)] = pnex_io_measure_duty(gpio);
            }
            pnex_io_write_pwm(p, 0);
            break;
        }
        default:
            break;
    }
    pnex_io_release(gpio);
    forget_pin(gpio);
    emit(doc);
}

// Splits `line` in place into at most `max` whitespace-separated tokens.
uint8_t tokenize(char* line, char** argv, uint8_t max) {
    uint8_t argc = 0;
    char* save = nullptr;
    for (char* t = strtok_r(line, " \t", &save); t && argc < max; t = strtok_r(nullptr, " \t", &save)) {
        argv[argc++] = t;
    }
    return argc;
}

bool parse_gpio(const char* s, uint8_t& out) {
    char* end = nullptr;
    const long v = strtol(s, &end, 10);
    if (end == s || *end != '\0' || v < 0 || v > 48) {
        return false;
    }
    out = (uint8_t)v;
    return true;
}

void handle_line(char* line) {
    char* argv[4] = {};
    const uint8_t argc = tokenize(line, argv, 4);
    if (argc == 0) {
        return;
    }
    const char* op = argv[0];
    if (strcmp(op, "info") == 0) {
        emit_info("info");
        return;
    }
    uint8_t gpio = 0;
    if (argc < 2 || !parse_gpio(argv[1], gpio)) {
        emit_error(op, "bad_gpio");
        return;
    }
    if (is_protected(gpio)) {
        emit_error(op, "protected_pin");
        return;
    }

    if (strcmp(op, "test") == 0 || strcmp(op, "mode") == 0) {
        const TestMode m = argc >= 3 ? parse_mode(argv[2]) : TM_UNKNOWN;
        if (m == TM_UNKNOWN) {
            emit_error(op, "bad_mode");
            return;
        }
        const bool safe_high = argc >= 4 && strcmp(argv[3], "safe_high") == 0;
        if (strcmp(op, "test") == 0) {
            run_test(gpio, m, safe_high, argv[2]);
            return;
        }
        PnexPin* p = upsert_pin(make_pin(gpio, m, safe_high));
        if (!p) {
            emit_error(op, "pin_table_full");
            return;
        }
        pnex_io_apply(*p);
        JsonDocument doc;
        doc["op"] = "mode";
        doc["gpio"] = gpio;
        doc["mode"] = argv[2];
        doc["level"] = p->mode == PNEX_ADC_IN ? -1 : pnex_io_level(gpio);
        emit(doc);
        return;
    }
    if (strcmp(op, "release") == 0) {
        pnex_io_release(gpio);
        forget_pin(gpio);
        JsonDocument doc;
        doc["op"] = "release";
        doc["gpio"] = gpio;
        emit(doc);
        return;
    }

    PnexPin* p = find_pin(gpio);
    if (!p) {
        emit_error(op, "pin_not_configured");
        return;
    }
    JsonDocument doc;
    doc["op"] = op;
    doc["gpio"] = gpio;
    if (strcmp(op, "read") == 0) {
        doc["mode"] = pnex_io_mode_name(p->mode);
        doc["value"] = pnex_io_read(*p);
    } else if (strcmp(op, "write") == 0) {
        if (p->mode != PNEX_DIGITAL_OUT || argc < 3) {
            emit_error(op, "not_digital_out");
            return;
        }
        const bool high = atoi(argv[2]) != 0;
        doc["value"] = high ? 1 : 0;
        doc["readback"] = pnex_io_write_digital(gpio, high);
    } else if (strcmp(op, "pwm") == 0) {
        if (p->mode != PNEX_PWM_OUT || argc < 3) {
            emit_error(op, "not_pwm_out");
            return;
        }
        pnex_io_write_pwm(*p, atoi(argv[2]));
        delay(5);
        doc["duty"] = p->duty_pct;
        doc["measured"] = pnex_io_measure_duty(gpio);
    } else {
        emit_error(op, "unknown_op");
        return;
    }
    emit(doc);
}

}  // namespace

void pnex_selftest_run() {
    Serial.begin(115200);
    delay(500);
    Serial.println("\n[selftest] PneX pin self-test console — type `info`");
    emit_info("ready");

    char line[64];
    uint8_t len = 0;
    for (;;) {
        while (Serial.available() > 0) {
            const char c = (char)Serial.read();
            if (c == '\r') {
                continue;
            }
            if (c == '\n') {
                line[len] = '\0';
                handle_line(line);
                len = 0;
            } else if (len < sizeof(line) - 1) {
                line[len++] = c;
            }
        }
        delay(1);  // also feeds the watchdogs
    }
}
