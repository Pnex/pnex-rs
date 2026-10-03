//
// PneX pin I/O primitives — see pnex_io.h.
//

#include "pnex_io.h"

#if defined(ESP32)
#include <driver/gpio.h>
#include <soc/gpio_periph.h>
#include <soc/io_mux_reg.h>
#include <soc/soc_caps.h>
#endif

// ESP8266 A0 = wire id 17 (ADC channel, no physical GPIO 17).
static const uint8_t ESP8266_A0_WIRE_ID = 17;

#if defined(ESP32)
// LEDC channels are allocated here, not through analogWrite(): the core's
// analogWrite takes one channel per pin for good and never frees it, so a
// C3 (6 channels) silently stopped driving its 7th PWM pin ever used —
// found by the D122 bench. Same scale as analogWrite (1 kHz, 8 bits),
// allocated from the top like analogWrite (low channels stay free for the
// camera XCLK).
#if defined(SOC_LEDC_SUPPORT_HS_MODE)
#define PNEX_LEDC_CHANNELS (SOC_LEDC_CHANNEL_NUM << 1)
#else
#define PNEX_LEDC_CHANNELS SOC_LEDC_CHANNEL_NUM
#endif
static const uint32_t PWM_FREQ_HZ = 1000;
static const uint8_t PWM_BITS = 8;
static const uint8_t NO_GPIO = 0xFF;
static uint8_t s_channel_gpio[PNEX_LEDC_CHANNELS];
static bool s_channels_init = false;

static int channel_of(uint8_t gpio) {
    if (!s_channels_init) {
        memset(s_channel_gpio, NO_GPIO, sizeof(s_channel_gpio));
        s_channels_init = true;
    }
    for (int ch = 0; ch < PNEX_LEDC_CHANNELS; ++ch) {
        if (s_channel_gpio[ch] == gpio) {
            return ch;
        }
    }
    return -1;
}

static int attach_pwm(uint8_t gpio) {
    int ch = channel_of(gpio);
    if (ch >= 0) {
        return ch;
    }
    for (ch = PNEX_LEDC_CHANNELS - 1; ch >= 0; --ch) {
        if (s_channel_gpio[ch] == NO_GPIO) {
            ledcSetup(ch, PWM_FREQ_HZ, PWM_BITS);
            ledcAttachPin(gpio, ch);
            s_channel_gpio[ch] = gpio;
            return ch;
        }
    }
    return -1;
}

// Frees the LEDC channel of a pin: the GPIO matrix keeps the signal routed
// across pinMode, so a pwm_out → digital_out switch would keep pulsing.
static void detach_pwm(uint8_t gpio) {
    const int ch = channel_of(gpio);
    if (ch >= 0) {
        ledcWrite(ch, 0);
        ledcDetachPin(gpio);
        s_channel_gpio[ch] = NO_GPIO;
    }
}
#endif

bool pnex_io_apply(PnexPin& p) {
#if defined(ESP8266)
    if (p.gpio == ESP8266_A0_WIRE_ID) {
        return true;  // ADC channel: no pinMode
    }
#else
    if (p.mode != PNEX_PWM_OUT) {
        detach_pwm(p.gpio);
    }
#endif
    switch (p.mode) {
        case PNEX_DIGITAL_OUT:
            digitalWrite(p.gpio, p.safe_high ? HIGH : LOW);  // safe level BEFORE driving
            pinMode(p.gpio, OUTPUT);
            break;
        case PNEX_PWM_OUT:
            p.duty_pct = 0;
#if defined(ESP32)
            if (attach_pwm(p.gpio) < 0) {
                Serial.printf("[IO] GPIO%u: no free PWM channel (%d in use)\n", p.gpio,
                              PNEX_LEDC_CHANNELS);
                return false;
            }
            ledcWrite(channel_of(p.gpio), 0);  // boot at duty 0 (safe)
#else
            analogWrite(p.gpio, 0);  // boot at duty 0 (safe)
#endif
            break;
        case PNEX_DIGITAL_IN:
            pinMode(p.gpio, p.pullup ? INPUT_PULLUP : INPUT);
            break;
        default:
            break;  // ADC: nothing to configure
    }
    return true;
}

int pnex_io_level(uint8_t gpio) {
#if defined(ESP32)
    // Make sure the pad input buffer is on (a LEDC-routed pad may have it
    // off) without touching the output routing.
    PIN_INPUT_ENABLE(GPIO_PIN_MUX_REG[gpio]);
    return gpio_get_level((gpio_num_t)gpio) ? 1 : 0;
#else
    return digitalRead(gpio) == HIGH ? 1 : 0;
#endif
}

int pnex_io_write_digital(uint8_t gpio, bool high) {
    digitalWrite(gpio, high ? HIGH : LOW);
    delayMicroseconds(50);  // let the pad settle before the read back
    return pnex_io_level(gpio);
}

void pnex_io_write_pwm(PnexPin& p, int duty_pct) {
    if (duty_pct < 0) duty_pct = 0;
    if (duty_pct > 100) duty_pct = 100;
    p.duty_pct = (uint8_t)duty_pct;
#if defined(ESP32)
    const int ch = attach_pwm(p.gpio);
    if (ch >= 0) {
        ledcWrite(ch, duty_pct * 255 / 100);
    }
#else
    analogWrite(p.gpio, duty_pct * 255 / 100);
#endif
}

int pnex_io_read(const PnexPin& p) {
    switch (p.mode) {
        case PNEX_ADC_IN:
#if defined(ESP8266)
            return analogRead(A0);
#else
            return analogRead(p.gpio);
#endif
        case PNEX_PWM_OUT:
            return p.duty_pct;
        default:
            return pnex_io_level(p.gpio);
    }
}

uint8_t pnex_io_measure_duty(uint8_t gpio) {
    // ~2000 samples, 5–20 µs apart (jitter breaks the aliasing with the
    // ~1 kHz PWM period) ≈ 25 periods.
    const uint16_t samples = 2000;
    uint16_t high = 0;
    for (uint16_t i = 0; i < samples; ++i) {
        high += pnex_io_level(gpio);
        delayMicroseconds(5 + (micros() % 16));
#if defined(ESP8266)
        if ((i & 0xFF) == 0) {
            yield();
        }
#endif
    }
    return (uint8_t)((high * 100UL + samples / 2) / samples);
}

void pnex_io_release(uint8_t gpio) {
#if defined(ESP8266)
    if (gpio == ESP8266_A0_WIRE_ID) {
        return;
    }
    analogWrite(gpio, 0);  // stops the waveform
#else
    detach_pwm(gpio);
#endif
    pinMode(gpio, INPUT);
}

const char* pnex_io_mode_name(uint8_t mode) {
    switch (mode) {
        case PNEX_DIGITAL_OUT:
            return "digital_out";
        case PNEX_PWM_OUT:
            return "pwm_out";
        case PNEX_ADC_IN:
            return "adc_in";
        default:
            return "digital_in";
    }
}
