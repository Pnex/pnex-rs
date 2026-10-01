// pnex_screen — implementation. See pnex_screen.h for the facade contract.
//
// The two driver implementations are mutually exclusive compile-time
// branches: at most one kind gate is 1 (the server resolves a single kind
// per device). Driver #includes sit INSIDE the gates so a screenless build
// never references (and PIO LDF never links) U8g2 or the Adafruit libs.
//
// ESP8266 + ST7735 uses a degraded direct-draw path: the full-frame
// GFXcanvas16 (160x128x2 = 40 KB) does not reliably fit the ESP8266 heap
// next to WiFi + websockets, so the splash is drawn once, beams included,
// straight to the panel — no animation, no reflection.
//
// V2 layout (pnex-tft-mockups.html, /3): the status bar (device id, wifi
// bars, server dot, TX/RX) lives at the bottom from boot on; the
// connection timeline (WiFi → WS → Reg) sits above it while connecting;
// after "Registered" (+1 s hold) the MAIN page shows the data placeholder.
// The page machine BOOT→CONNEXION→MAIN lives here and only here; network
// state is PULLED from pnex_status at each tick. All drawing is
// change-only / partial (except the animated canvas frames while
// connecting, which repaint the whole canvas anyway).

#include "pnex_screen.h"

#include "pnex_status.h"

// Driver headers live at FILE SCOPE (a #include inside the namespace would
// wrap the whole display library in pnex_screen) but INSIDE the kind gates:
// a screenless build references nothing and PIO LDF links no display lib.
#if PNEX_SCREEN_SSD1306 == 1
#include <U8g2lib.h>
#endif
#if PNEX_SCREEN_ST7735 == 1
#include <SPI.h>
#include <Adafruit_GFX.h>
#include <Adafruit_ST7735.h>
#endif

// ─────────────── Gate plumbing (defaults for manual builds) ───────────────

#ifndef PNEX_SCREEN_SSD1306
#define PNEX_SCREEN_SSD1306 0
#endif
#ifndef PNEX_SCREEN_ST7735
#define PNEX_SCREEN_ST7735 0
#endif
#ifndef PNEX_SCREEN_SDA
#define PNEX_SCREEN_SDA (-1)
#endif
#ifndef PNEX_SCREEN_SCL
#define PNEX_SCREEN_SCL (-1)
#endif
#ifndef PNEX_SCREEN_SCK
#define PNEX_SCREEN_SCK (-1)
#endif
#ifndef PNEX_SCREEN_MOSI
#define PNEX_SCREEN_MOSI (-1)
#endif
#ifndef PNEX_SCREEN_CS
#define PNEX_SCREEN_CS (-1)
#endif
#ifndef PNEX_SCREEN_DC
#define PNEX_SCREEN_DC (-1)
#endif
#ifndef PNEX_SCREEN_RST
#define PNEX_SCREEN_RST (-1)
#endif

namespace pnex_screen {

// Device id shared by both drivers (copied — the transport buffer churns).
static char s_device_id[32] = "";
static bool s_id_dirty = true;  // bar id needs a redraw (OLED partial path)

void set_device_id(const char* id) {
    if (!id) {
        return;
    }
    snprintf(s_device_id, sizeof(s_device_id), "%s", id);
    s_id_dirty = true;
}

// ──────────── Shared screen-side state (both drivers, V2 grammar) ────────────

#if PNEX_HAS_SCREEN

constexpr unsigned long REGISTERED_HOLD_MS = 1000;  // full timeline → MAIN
constexpr unsigned long TX_RX_LIT_MS = 150;         // arrow lit per traffic
constexpr unsigned long BLINK_MS = 400;             // 800 ms blink period
constexpr unsigned long OTA_FAILED_HOLD_MS = 8000;  // "failed" → back to MAIN

static uint8_t stepIdx(const pnex_status::NetState& st) {
    return st.step == pnex_status::Step::Wifi
               ? 0
               : (st.step == pnex_status::Step::Link ? 1 : 2);
}

static bool blinkOn(unsigned long now) {
    return (now / BLINK_MS) % 2 == 0;
}

// TX/RX activity: the network side only bumps counters; "lit" is a short
// window after the last change, computed screen-side from millis().
static uint32_t s_seen_tx = 0, s_seen_rx = 0;
static unsigned long s_tx_ms = 0, s_rx_ms = 0;

static void noteTraffic(const pnex_status::NetState& st, unsigned long now) {
    if (st.tx_count != s_seen_tx) {
        s_seen_tx = st.tx_count;
        s_tx_ms = now;
    }
    if (st.rx_count != s_seen_rx) {
        s_seen_rx = st.rx_count;
        s_rx_ms = now;
    }
}

static bool txOn(unsigned long now) {
    return s_tx_ms != 0 && now - s_tx_ms < TX_RX_LIT_MS;
}

static bool rxOn(unsigned long now) {
    return s_rx_ms != 0 && now - s_rx_ms < TX_RX_LIT_MS;
}

// Server dot: 1 = link OK, 2 = lost (was up once), 0 = not yet.
static bool s_ever_link = false;

static uint8_t dotMode(const pnex_status::NetState& st) {
    if (st.link_ok) {
        return 1;
    }
    return s_ever_link ? 2 : 0;
}

// Live sample: adc → analogRead, pwm → snapshot duty (not readable back),
// else digitalRead (outputs read their own level).
static uint32_t samplePin(const pnex_status::PinEntry& e) {
    switch (e.mode) {
        case pnex_status::PinMode::AdcIn:
            return (uint32_t)analogRead(e.gpio);
        case pnex_status::PinMode::PwmOut:
            return e.duty_pct;
        default:
            return (uint32_t)digitalRead(e.gpio);
    }
}

// Value cell text (≤ 4 chars: "4095" raw ADC / "100%" duty / "1").
static void formatPinValue(char* buf, size_t n, const pnex_status::PinEntry& e,
                           uint32_t v) {
    if (e.mode == pnex_status::PinMode::PwmOut) {
        snprintf(buf, n, "%u%%", (unsigned)v);
    } else {
        snprintf(buf, n, "%u", (unsigned)v);
    }
}

// OTA progress states (pnex_status phase strings).
enum class OtaPhase : uint8_t { Downloading = 0, Flashing = 1, Failed = 2 };

static OtaPhase otaPhaseIdx(const pnex_status::NetState& st) {
    if (strcmp(st.ota_phase, "flashing") == 0) {
        return OtaPhase::Flashing;
    }
    if (strcmp(st.ota_phase, "failed") == 0) {
        return OtaPhase::Failed;
    }
    return OtaPhase::Downloading;
}

// Level trigger for the OTA page (failed alone never re-enters it).
static bool otaActive(const pnex_status::NetState& st) {
    return strcmp(st.ota_phase, "downloading") == 0 ||
           strcmp(st.ota_phase, "flashing") == 0;
}

// Page machine (screen-side only): Connexion(wifi|ws|reg) → Main ⇄ Ota.
enum class Page : uint8_t { Connexion, Main, Ota };

static Page s_page = Page::Connexion;
static bool s_reg_seen = false;
static unsigned long s_reg_ms = 0;

// Connexion page: true once "Registered" has been held long enough.
static bool registeredHeld(const pnex_status::NetState& st, unsigned long now) {
    if (st.step == pnex_status::Step::Registered && !s_reg_seen) {
        s_reg_seen = true;
        s_reg_ms = now;
    }
    return s_reg_seen && now - s_reg_ms >= REGISTERED_HOLD_MS;
}

#endif  // PNEX_HAS_SCREEN

// ────────────────────────── SSD1306 OLED (I2C) ──────────────────────────

#if PNEX_SCREEN_SSD1306 == 1

// V2 port of the TFT layout to the 128x64 mono panel. Every zone sits on
// whole 8-px bands (U8g2 tiles) so partial pushes stay minimal:
//   band 0..3  Tron gerbe + logo pill          (Connexion page)
//   band 4..5  timeline WiFi ─ WS ─ Reg        (Connexion page)
//   band 0..5  pin panel / OTA progress        (Main / Ota pages)
//   band 6..7  status bar: id, page dots, wifi bars, server dot, TX / RX
//
// Two-color panels (yellow rows 48..63 once rotated, blue above) exist next
// to mono ones: page content never crosses ZONE_SPLIT_Y, only the status
// bar lives in the bottom band, so both variants look clean.
//
// Bus: U8g2 SW I2C (bit-bang). HW I2C (Wire) is unreliable on the NodeMCU V3
// OLED: bus stuck at 400 kHz, corrupted bytes (noise) at 100 kHz. SW I2C is
// clean but slow (~180 us per byte, ~190 ms per full frame), so the full
// frame lives in the U8g2 buffer (always the truth) and flush() only pushes
// the dirty tile rectangles (updateDisplayArea): the gerbe around the logo
// pill, one timeline node, the TX/RX arrows, one pin cell...

static U8G2_SSD1306_128X64_NONAME_F_SW_I2C* u8g2 = nullptr;

constexpr int OLED_W = 128;
constexpr int OLED_H = 64;
constexpr int OLED_TILES_X = OLED_W / 8;
constexpr int OLED_TILES_Y = OLED_H / 8;
// The panel is mounted 180° on the NodeMCU V3 OLED (U8G2_R2): logical
// (x, y) lives at buffer (W-1-x, H-1-y). Bands stay tile-aligned (64 and
// 128 are multiples of 8).
constexpr bool OLED_FLIPPED = true;

constexpr int ZONE_SPLIT_Y = 48;  // two-color panels: first yellow row
constexpr int BAR_SEP_Y = 48;     // separator above the status bar (first yellow row)
constexpr int BAR_BASE = 63;      // bottom row of the bar icons
constexpr unsigned long PIN_REFRESH_MS = 200;
constexpr unsigned long PIN_PAGE_MS = 3000;  // pin panel page rotation
constexpr int PIN_ROWS = 6;                  // one band per row, bands 0..5
constexpr int PIN_PER_PAGE = PIN_ROWS * 2;
constexpr int PIN_CELLS_PER_PASS = 3;  // cap the SW I2C cost of one pass
constexpr uint32_t ADC_HYSTERESIS = 8;  // raw ADC jitter kept off the bus


// ── Dirty tile rectangles ──

static uint16_t s_dirty[OLED_TILES_Y];  // per buffer tile row: column bits
static bool s_dirty_all = false;

// Mark the logical rectangle [x0..x1] × [y0..y1] (inclusive) dirty.
static void markDirty(int x0, int y0, int x1, int y1) {
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > OLED_W - 1) x1 = OLED_W - 1;
    if (y1 > OLED_H - 1) y1 = OLED_H - 1;
    if (x0 > x1 || y0 > y1) {
        return;
    }
    if (OLED_FLIPPED) {
        const int bx0 = OLED_W - 1 - x1, bx1 = OLED_W - 1 - x0;
        const int by0 = OLED_H - 1 - y1, by1 = OLED_H - 1 - y0;
        x0 = bx0;
        x1 = bx1;
        y0 = by0;
        y1 = by1;
    }
    uint16_t cols = 0;
    for (int tx = x0 / 8; tx <= x1 / 8; tx++) {
        cols |= (uint16_t)(1u << tx);
    }
    for (int ty = y0 / 8; ty <= y1 / 8; ty++) {
        s_dirty[ty] |= cols;
    }
}

static void markAll() {
    s_dirty_all = true;
}

// Push the dirty tiles: whole frame when everything changed, else one
// updateDisplayArea per contiguous column run of each tile row.
static void flush() {
    if (s_dirty_all) {
        u8g2->sendBuffer();
        s_dirty_all = false;
        for (int ty = 0; ty < OLED_TILES_Y; ty++) {
            s_dirty[ty] = 0;
        }
        return;
    }
    for (int ty = 0; ty < OLED_TILES_Y; ty++) {
        const uint16_t cols = s_dirty[ty];
        if (cols == 0) {
            continue;
        }
        int tx = 0;
        while (tx < OLED_TILES_X) {
            if (!(cols & (1u << tx))) {
                tx++;
                continue;
            }
            int end = tx;
            while (end + 1 < OLED_TILES_X && (cols & (1u << (end + 1)))) {
                end++;
            }
            u8g2->updateDisplayArea(tx, ty, end - tx + 1, 1);
            tx = end + 1;
        }
        s_dirty[ty] = 0;
    }
}

static void clearBox(int x, int y, int w, int h) {
    u8g2->setDrawColor(0);
    u8g2->drawBox(x, y, w, h);
    u8g2->setDrawColor(1);
}

static void drawCentered(const char* s, int cx, int baseline) {
    u8g2->drawStr(cx - u8g2->getStrWidth(s) / 2, baseline, s);
}

// Diagonal hatch of period 6 moving right with `phase` (pattern f(x - t)).
static void drawHatch(int x0, int y0, int x1, int y1, int phase) {
    for (int y = y0; y <= y1; y++) {
        for (int x = x0; x <= x1; x++) {
            if ((x + y + 60 - phase) % 6 < 3) {
                u8g2->drawPixel(x, y);
            }
        }
    }
}

// ── Status bar ──

// Up (TX ▲) / down (RX ▼) triangle, TRI_W px wide × TRI_H px tall;
// idle = outline, traffic = filled (large enough for the fill to read).
constexpr int TRI_H = 7;
constexpr int TRI_W = TRI_H * 2 - 1;
static void drawTri(int x, int y, bool up, bool filled) {
    for (int row = 0; row < TRI_H; row++) {
        const int width = 1 + 2 * (up ? row : TRI_H - 1 - row);
        const int x0 = x + (TRI_W - width) / 2;
        const bool edge_row = up ? (row == TRI_H - 1) : (row == 0);
        if (filled || edge_row || width == 1) {
            u8g2->drawHLine(x0, y + row, width);
        } else {
            u8g2->drawPixel(x0, y + row);
            u8g2->drawPixel(x0 + width - 1, y + row);
        }
    }
}

// Current pin panel page (MAIN) — its dots are drawn in the bar.
static uint8_t s_pin_page = 0;

// Bar sections, each redrawn and pushed on its own change only.
constexpr int BAR_ID_X1 = 75;                 // id + page dots
constexpr int BAR_WIFI_X0 = 78, BAR_WIFI_X1 = 88;
constexpr int BAR_DOT_X0 = 91, BAR_DOT_X1 = 97;
constexpr int BAR_TRX_X0 = 100, BAR_TRX_X1 = 127;  // two TRI_W arrows
constexpr int BAR_ICON_Y0 = 50;               // icons band top

static uint8_t s_bar_lvl = 255, s_bar_dot = 255, s_bar_trx = 255;
static uint8_t s_bar_pages = 255, s_bar_page = 255;
static bool s_bar_built = false;

static void drawBarId(const pnex_status::NetState& st) {
    clearBox(0, BAR_ICON_Y0, BAR_ID_X1 + 1, OLED_H - BAR_ICON_Y0);
    u8g2->setFont(u8g2_font_5x7_tr);
    char buf[13];  // 12 chars of 5 px
    snprintf(buf, sizeof(buf), "%.12s", s_device_id);
    u8g2->drawStr(0, BAR_BASE - 1, buf);
    // Pin panel page dots (MAIN only, when paged): current filled 3x3,
    // others a single pixel — kept in the bar so the page content stays
    // above ZONE_SPLIT_Y on two-color panels.
    const int pages = (st.pin_count + PIN_PER_PAGE - 1) / PIN_PER_PAGE;
    if (s_page == Page::Main && pages > 1) {
        for (int p = 0; p < pages; p++) {
            const int px = 63 + p * 4;
            if (p == s_pin_page) {
                u8g2->drawBox(px, BAR_BASE - 4, 3, 3);
            } else {
                u8g2->drawPixel(px + 1, BAR_BASE - 3);
            }
        }
    }
    markDirty(0, BAR_ICON_Y0, BAR_ID_X1, OLED_H - 1);
}

// WiFi: 4 bars, 2 px wide, heights 2/4/6/8; off = 1 px floor tick.
static void drawBarWifi(uint8_t lvl) {
    clearBox(BAR_WIFI_X0, BAR_ICON_Y0, BAR_WIFI_X1 - BAR_WIFI_X0 + 1,
             OLED_H - BAR_ICON_Y0);
    static const uint8_t h[4] = {2, 4, 6, 8};
    for (int i = 0; i < 4; i++) {
        const int x = BAR_WIFI_X0 + i * 3;
        if (lvl > i) {
            u8g2->drawBox(x, BAR_BASE - h[i] + 1, 2, h[i]);
        } else {
            u8g2->drawHLine(x, BAR_BASE, 2);
        }
    }
    markDirty(BAR_WIFI_X0, BAR_ICON_Y0, BAR_WIFI_X1, OLED_H - 1);
}

// Server dot: filled = OK, cross = lost, ring = not yet.
static void drawBarDot(uint8_t mode) {
    clearBox(BAR_DOT_X0, BAR_ICON_Y0, BAR_DOT_X1 - BAR_DOT_X0 + 1,
             OLED_H - BAR_ICON_Y0);
    const int cx = 94, cy = BAR_BASE - 3;
    switch (mode) {
        case 1:
            u8g2->drawDisc(cx, cy, 3);
            break;
        case 2:
            u8g2->drawLine(cx - 3, cy - 3, cx + 3, cy + 3);
            u8g2->drawLine(cx - 3, cy + 3, cx + 3, cy - 3);
            break;
        default:
            u8g2->drawCircle(cx, cy, 3);
            break;
    }
    markDirty(BAR_DOT_X0, BAR_ICON_Y0, BAR_DOT_X1, OLED_H - 1);
}

static void drawBarTrx(bool tx, bool rx) {
    // Only the arrow rows are cleared/pushed: one tile row on SW I2C.
    const int y0 = BAR_BASE - TRI_H + 1;
    clearBox(BAR_TRX_X0, y0, BAR_TRX_X1 - BAR_TRX_X0 + 1, OLED_H - y0);
    drawTri(BAR_TRX_X0, y0, true, tx);
    drawTri(BAR_TRX_X1 - TRI_W + 1, y0, false, rx);
    markDirty(BAR_TRX_X0, y0, BAR_TRX_X1, OLED_H - 1);
}

// Section-wise bar pass: each section redraws on its own change only.
static void tickBar(const pnex_status::NetState& st, unsigned long now) {
    if (!s_bar_built) {
        clearBox(0, BAR_SEP_Y, OLED_W, OLED_H - BAR_SEP_Y);
        u8g2->drawHLine(0, BAR_SEP_Y, OLED_W);
        markDirty(0, BAR_SEP_Y, OLED_W - 1, BAR_SEP_Y);
        s_bar_built = true;
        s_id_dirty = true;
        s_bar_lvl = s_bar_dot = s_bar_trx = 255;
    }
    const uint8_t pages =
        (uint8_t)((st.pin_count + PIN_PER_PAGE - 1) / PIN_PER_PAGE);
    const uint8_t page = (s_page == Page::Main) ? s_pin_page : 255;
    if (s_id_dirty || pages != s_bar_pages || page != s_bar_page) {
        s_id_dirty = false;
        s_bar_pages = pages;
        s_bar_page = page;
        drawBarId(st);
    }
    const uint8_t lvl = st.wifi_ok ? st.wifi_level : 0;
    if (lvl != s_bar_lvl) {
        s_bar_lvl = lvl;
        drawBarWifi(lvl);
    }
    const uint8_t dot = dotMode(st);
    if (dot != s_bar_dot) {
        s_bar_dot = dot;
        drawBarDot(dot);
    }
    const bool tx = txOn(now), rx = rxOn(now);
    const uint8_t trx = (uint8_t)((tx ? 1 : 0) | (rx ? 2 : 0));
    if (trx != s_bar_trx) {
        s_bar_trx = trx;
        drawBarTrx(tx, rx);
    }
}

// ── Connexion page (TFT splash port): Tron gerbe + logo pill, timeline ──
//   bands 0..3  beams converging toward a focus above the screen, dotted
//               floor, wordmark knocked out of a white pill (TFT pill echo)
//   bands 4..5  timeline WiFi ─ WS ─ Reg: validated = disc, current =
//               blinking disc/ring, upcoming = ring; solid track up to the
//               reached node, dotted after.
// The gerbe repaints every GERBE_MS (~8 fps: SW I2C budget) and only its
// tiles outside the pill are pushed.

constexpr int GERBE_BEAMS = 12;
constexpr float GERBE_FOCUS_X = OLED_W / 2.0f;
constexpr float GERBE_FOCUS_Y = -8.0f;
constexpr float GERBE_CTRL_Y = 14.0f;  // bezier control height (TFT 52 scaled)
constexpr int GERBE_FLOOR_Y = 31;      // last gerbe row (bottom of band 3)
constexpr float GERBE_SEG_SPEED = 0.24f;  // same pace as the TFT
constexpr float GERBE_SEG_MIN = 0.18f, GERBE_SEG_MAX = 0.50f;
constexpr int GERBE_STEPS = 48;  // bezier samples along a beam
constexpr unsigned long GERBE_MS = 120;

// Logo pill (white rounded box, wordmark drawn in black inside).
constexpr int PILL_X = 30, PILL_Y = 4, PILL_W = 68, PILL_H = 25;

constexpr int TL_Y = 36;                   // track row (band 4)
constexpr int TL_R = 3;                    // node radius
constexpr int TL_X[3] = {16, 64, 112};     // node centers
constexpr int TL_LABEL_Y = 46;             // labels baseline (band 5)
static const char* const TL_LABELS[3] = {"WiFi", "WS", "Reg"};

struct GerbeSeg {
    float head;
    float len;
};
struct GerbeBeam {
    float entry_x;
    GerbeSeg segs[2];
};
static GerbeBeam s_beams[GERBE_BEAMS];

static float rand01() { return random(1000) / 1000.0f; }

static void initGerbe() {
    for (int i = 0; i < GERBE_BEAMS; i++) {
        s_beams[i].entry_x =
            OLED_W / 2.0f + (i - (GERBE_BEAMS - 1) / 2.0f) * 11.0f;
        for (int s = 0; s < 2; s++) {
            s_beams[i].segs[s].head = rand01() * 1.5f - 0.5f;
            s_beams[i].segs[s].len =
                GERBE_SEG_MIN + rand01() * (GERBE_SEG_MAX - GERBE_SEG_MIN);
        }
    }
}

// Quadratic bezier floor → focus, control above the focus column.
static void gerbePoint(const GerbeBeam& b, float u, int& x, int& y) {
    const float w0 = (1.0f - u) * (1.0f - u);
    const float w1 = 2.0f * (1.0f - u) * u;
    const float w2 = u * u;
    x = (int)(w0 * b.entry_x + w1 * GERBE_FOCUS_X + w2 * GERBE_FOCUS_X);
    y = (int)(w0 * GERBE_FLOOR_Y + w1 * GERBE_CTRL_Y + w2 * GERBE_FOCUS_Y);
}

// Segments glide toward the focus; one that fully exits is reborn below
// the floor with a fresh random length.
static void updateGerbe() {
    static unsigned long last = 0;
    const unsigned long now = millis();
    const float dt = (now - last) / 1000.0f;
    last = now;
    if (dt <= 0.0f || dt > 0.5f) {
        return;  // boot jump / long blocking wait (TLS handshake)
    }
    for (int i = 0; i < GERBE_BEAMS; i++) {
        for (int s = 0; s < 2; s++) {
            GerbeSeg& sg = s_beams[i].segs[s];
            sg.head += GERBE_SEG_SPEED * dt;
            if (sg.head - sg.len > 1.0f) {
                sg.head = -rand01() * 0.5f;
                sg.len = GERBE_SEG_MIN + rand01() * (GERBE_SEG_MAX - GERBE_SEG_MIN);
            }
        }
    }
}

// White pill + wordmark in black: bold "pne" + the x as a thick cross with
// its dot punched back in white.
static void drawLogoPill() {
    u8g2->drawRBox(PILL_X, PILL_Y, PILL_W, PILL_H, 5);
    u8g2->setDrawColor(0);
    u8g2->setFont(u8g2_font_ncenB14_tr);
    const int tw = u8g2->getStrWidth("pne");
    const int gs = 13;
    const int total = tw + 3 + gs + 4;
    const int tx = PILL_X + (PILL_W - total) / 2, ty = PILL_Y + 17;
    u8g2->setFontMode(1);
    u8g2->drawStr(tx, ty, "pne");
    const int gx = tx + tw + 3, gy = ty - gs + 1;
    for (int i = 0; i <= gs; i++) {
        for (int w = 0; w < 4; w++) {
            u8g2->drawPixel(gx + i + w, gy + i);
            u8g2->drawPixel(gx + gs - i + w, gy + i);
        }
    }
    u8g2->setDrawColor(1);
    u8g2->drawDisc(gx + gs / 2 + 2, gy + gs / 2, 2);
    u8g2->setFontMode(0);
}

// One gerbe frame into bands 0..3, pill on top. Pushes everything except
// the tiles fully covered by the pill (static).
static void drawGerbe() {
    updateGerbe();
    clearBox(0, 0, OLED_W, GERBE_FLOOR_Y + 1);
    for (int x = 0; x < OLED_W; x += 2) {
        u8g2->drawPixel(x, GERBE_FLOOR_Y);  // dotted floor
    }
    for (int i = 0; i < GERBE_BEAMS; i++) {
        const GerbeBeam& b = s_beams[i];
        for (int s = 0; s < 2; s++) {
            const GerbeSeg& sg = b.segs[s];
            int from = (int)((sg.head - sg.len) * GERBE_STEPS);
            int to = (int)(sg.head * GERBE_STEPS);
            if (from < 0) from = 0;
            if (to > GERBE_STEPS) to = GERBE_STEPS;
            int px = -1, py = -1;
            for (int st = from; st <= to; st++) {
                int x, y;
                gerbePoint(b, st / (float)GERBE_STEPS, x, y);
                if (y < 0 || y >= GERBE_FLOOR_Y) {
                    px = -1;
                    continue;
                }
                if (px >= 0) {
                    u8g2->drawLine(px, py, x, y);
                } else {
                    u8g2->drawPixel(x, y);
                }
                px = x;
                py = y;
            }
        }
    }
    drawLogoPill();
    // Dirty: bands 0 and 3 full; bands 1..2 only left/right of the pill.
    markDirty(0, 0, OLED_W - 1, 7);
    markDirty(0, 24, OLED_W - 1, 31);
    markDirty(0, 8, PILL_X + 7, 23);
    markDirty(PILL_X + PILL_W - 8, 8, OLED_W - 1, 23);
}

static uint8_t s_tl_idx = 255;   // step drawn (255 = rebuild)
static bool s_tl_blink = false;  // blink state drawn
static unsigned long s_gerbe_ms = 0;

static void drawTlNode(int i, bool filled) {
    // Clear the circle bounds only: the track stays continuous up to the
    // node edge, and a ring is empty inside (same as the full draw).
    clearBox(TL_X[i] - TL_R, TL_Y - TL_R, 2 * TL_R + 1, 2 * TL_R + 1);
    if (filled) {
        u8g2->drawDisc(TL_X[i], TL_Y, TL_R);
    } else {
        u8g2->drawCircle(TL_X[i], TL_Y, TL_R);
    }
    markDirty(TL_X[i] - TL_R - 1, TL_Y - TL_R - 1, TL_X[i] + TL_R + 1,
              TL_Y + TL_R + 1);
}

// Whole timeline (bands 4..5): track, nodes, labels.
static void drawTimelineFull(const pnex_status::NetState& st, bool blink) {
    const bool reg = (st.step == pnex_status::Step::Registered);
    const uint8_t idx = reg ? 3 : stepIdx(st);
    clearBox(0, 32, OLED_W, 16);
    const int reached = TL_X[idx > 2 ? 2 : idx];
    u8g2->drawHLine(TL_X[0], TL_Y, reached - TL_X[0]);
    for (int x = reached; x <= TL_X[2]; x += 2) {
        u8g2->drawPixel(x, TL_Y);  // dotted: not reached yet
    }
    u8g2->setFont(u8g2_font_5x7_tr);
    for (int i = 0; i < 3; i++) {
        drawTlNode(i, i < idx || (i == idx && blink));
        drawCentered(TL_LABELS[i], TL_X[i], TL_LABEL_Y);
    }
    markDirty(0, 32, OLED_W - 1, 47);
    s_tl_idx = idx;
    s_tl_blink = blink;
}

// Timeline pass: full redraw on step change, else only the current node
// on a blink edge.
static void tickTimeline(const pnex_status::NetState& st, unsigned long now) {
    const bool reg = (st.step == pnex_status::Step::Registered);
    const uint8_t idx = reg ? 3 : stepIdx(st);
    const bool blink = blinkOn(now);
    if (idx != s_tl_idx) {
        drawTimelineFull(st, blink);
        return;
    }
    if (idx < 3 && blink != s_tl_blink) {
        s_tl_blink = blink;
        drawTlNode(idx, blink);
    }
}

static void enterConnexion(const pnex_status::NetState& st, unsigned long now) {
    u8g2->clearBuffer();
    initGerbe();
    drawGerbe();
    drawTimelineFull(st, blinkOn(now));
    s_bar_built = false;
    tickBar(st, now);
    markAll();
    s_gerbe_ms = now;
}

static void tickConnexion(const pnex_status::NetState& st, unsigned long now) {
    tickTimeline(st, now);
    if (now - s_gerbe_ms >= GERBE_MS) {
        s_gerbe_ms = now;
        drawGerbe();
    }
}

// ── MAIN page: live pin panel, 2 columns × 6 rows, paged every 3 s ──

struct PinCell {
    uint8_t gpio = 255;
    uint8_t mode = 255;
    uint32_t value = 0;
    bool shown = false;  // something is drawn in this slot
};
static PinCell s_pin_cells[PIN_PER_PAGE];
static uint8_t s_pin_count = 255;  // 255 = panel not built yet
static unsigned long s_pins_ms = 0, s_pin_page_ms = 0;

// One cell = one band × half width (8 tiles, ~12 ms on SW I2C).
static void drawPinCell(int slot, const pnex_status::PinEntry& e, uint32_t v) {
    const int x = (slot / PIN_ROWS) * 64;
    const int y = (slot % PIN_ROWS) * 8;
    clearBox(x, y, 64, 8);
    u8g2->setFont(u8g2_font_5x7_tr);
    char lab[9];
    snprintf(lab, sizeof(lab), "%.7s", e.label);
    u8g2->drawStr(x + 1, y + 6, lab);  // descender row = y + 7
    char vb[8];
    formatPinValue(vb, sizeof(vb), e, v);
    u8g2->drawStr(x + 62 - u8g2->getStrWidth(vb), y + 6, vb);
    markDirty(x, y, x + 63, y + 7);
}

// Wipe the panel zone (page flip / table republish / empty table).
static void resetPinPanel(const pnex_status::NetState& st) {
    clearBox(0, 0, OLED_W, ZONE_SPLIT_Y);
    if (st.pin_count == 0) {
        u8g2->setFont(u8g2_font_6x10_tr);
        drawCentered("- no pins -", OLED_W / 2, 28);
    }
    markDirty(0, 0, OLED_W - 1, ZONE_SPLIT_Y - 1);
    for (int i = 0; i < PIN_PER_PAGE; i++) {
        s_pin_cells[i] = PinCell{};
    }
}

static bool pinValueChanged(const PinCell& c, const pnex_status::PinEntry& e,
                            uint32_t v) {
    if (c.gpio != e.gpio || c.mode != (uint8_t)e.mode) {
        return true;
    }
    if (e.mode == pnex_status::PinMode::AdcIn) {
        const uint32_t d = v > c.value ? v - c.value : c.value - v;
        return d >= ADC_HYSTERESIS;
    }
    return c.value != v;
}

// Throttled pass: sample the current page, redraw at most
// PIN_CELLS_PER_PASS changed cells (the rest follow on the next passes).
static void tickPins(const pnex_status::NetState& st, unsigned long now) {
    if (now - s_pins_ms < PIN_REFRESH_MS) {
        return;
    }
    s_pins_ms = now;
    if (st.pin_count != s_pin_count) {
        s_pin_count = st.pin_count;
        s_pin_page = 0;
        s_pin_page_ms = now;
        resetPinPanel(st);
        return;  // cells fill in over the next passes
    }
    const int pages = (st.pin_count + PIN_PER_PAGE - 1) / PIN_PER_PAGE;
    const bool flip = pages > 1 && now - s_pin_page_ms >= PIN_PAGE_MS;
    if (flip) {
        s_pin_page = (uint8_t)((s_pin_page + 1) % pages);
        s_pin_page_ms = now;
    }
    const int first = s_pin_page * PIN_PER_PAGE;
    int n = st.pin_count - first;
    if (n > PIN_PER_PAGE) {
        n = PIN_PER_PAGE;
    }
    if (flip) {
        // No full wipe (~140 ms on SW I2C): cells repaint in place, and
        // slots the new page does not use are cleared, both spread over the
        // next passes by the budget below.
        for (int i = 0; i < PIN_PER_PAGE; i++) {
            const bool shown = s_pin_cells[i].shown;
            s_pin_cells[i] = PinCell{};
            s_pin_cells[i].shown = shown;
        }
    }
    int budget = PIN_CELLS_PER_PASS;
    for (int i = n; i < PIN_PER_PAGE && budget > 0; i++) {
        PinCell& c = s_pin_cells[i];
        if (c.shown) {
            const int x = (i / PIN_ROWS) * 64, y = (i % PIN_ROWS) * 8;
            clearBox(x, y, 64, 8);
            markDirty(x, y, x + 63, y + 7);
            c.shown = false;
            budget--;
        }
    }
    for (int i = 0; i < n && budget > 0; i++) {
        const pnex_status::PinEntry& e = st.pins[first + i];
        const uint32_t v = samplePin(e);
        PinCell& c = s_pin_cells[i];
        if (!pinValueChanged(c, e, v)) {
            continue;
        }
        c.gpio = e.gpio;
        c.mode = (uint8_t)e.mode;
        c.value = v;
        c.shown = true;
        drawPinCell(i, e, v);
        budget--;
    }
}

static void enterMain(const pnex_status::NetState& st, unsigned long now) {
    s_pin_count = 255;  // force the panel reset on the first pass
    s_pins_ms = 0;
    tickPins(st, now);
}

// ── OTA page: pct + progress bar + phase line; failed holds then MAIN ──
//   bands 0..1 title, 1..3 pct, 4 bar, 5 phase line.

constexpr int OTA_BAR_X = 4, OTA_BAR_Y = 32, OTA_BAR_W = 120, OTA_BAR_H = 8;
constexpr unsigned long OTA_ANIM_MS = 140;  // flashing hatch period

static unsigned long s_ota_failed_ms = 0;
static uint8_t s_ota_phase = 255;  // phase drawn (255 = rebuild)
static uint8_t s_ota_pct = 255;    // pct drawn
static unsigned long s_ota_anim_ms = 0;

static void drawOtaPct(OtaPhase ph, uint8_t pct) {
    clearBox(0, 12, OLED_W, 20);
    char buf[8];
    if (ph == OtaPhase::Failed) {
        snprintf(buf, sizeof(buf), "failed");
    } else {
        snprintf(buf, sizeof(buf), "%u%%", pct);
    }
    u8g2->setFont(u8g2_font_ncenB14_tr);
    const int w = u8g2->getStrWidth(buf);
    drawCentered(buf, OLED_W / 2, 28);
    markDirty(OLED_W / 2 - w / 2 - 8, 12, OLED_W / 2 + w / 2 + 8, 31);
}

// Bar interior: proportional while downloading, hatched while flashing (no
// progress there), empty when failed.
static void drawOtaBar(OtaPhase ph, uint8_t pct, unsigned long now) {
    const int x0 = OTA_BAR_X + 2, x1 = OTA_BAR_X + OTA_BAR_W - 3;
    const int y0 = OTA_BAR_Y + 2, y1 = OTA_BAR_Y + OTA_BAR_H - 3;
    clearBox(x0, y0, x1 - x0 + 1, y1 - y0 + 1);
    if (ph == OtaPhase::Downloading) {
        const int fill = (x1 - x0 + 1) * pct / 100;
        if (fill > 0) {
            u8g2->drawBox(x0, y0, fill, y1 - y0 + 1);
        }
    } else if (ph == OtaPhase::Flashing) {
        drawHatch(x0, y0, x1, y1, (int)((now / OTA_ANIM_MS) % 6));
    }
    markDirty(x0, y0, x1, y1);
}

// Phase change: whole zone (title, pct, bar frame, phase line).
static void drawOtaFull(const pnex_status::NetState& st, OtaPhase ph,
                        unsigned long now) {
    clearBox(0, 0, OLED_W, ZONE_SPLIT_Y);
    u8g2->setFont(u8g2_font_6x10_tr);
    drawCentered("OTA update", OLED_W / 2, 9);
    u8g2->drawFrame(OTA_BAR_X, OTA_BAR_Y, OTA_BAR_W, OTA_BAR_H);
    u8g2->setFont(u8g2_font_5x7_tr);
    static const char* const LB[3] = {"downloading", "flashing",
                                      "old firmware kept"};
    // Baseline 46: no descender in these labels, bottom row 46 < ZONE_SPLIT_Y.
    drawCentered(LB[(int)ph], OLED_W / 2, 46);
    markDirty(0, 0, OLED_W - 1, ZONE_SPLIT_Y - 1);
    drawOtaPct(ph, st.ota_progress);
    drawOtaBar(ph, st.ota_progress, now);
}

static void enterOta() {
    s_ota_failed_ms = 0;
    s_ota_phase = 255;
    s_ota_pct = 255;
}

static void tickOta(const pnex_status::NetState& st, unsigned long now) {
    const OtaPhase ph = otaPhaseIdx(st);
    if ((uint8_t)ph != s_ota_phase) {
        s_ota_phase = (uint8_t)ph;
        s_ota_pct = st.ota_progress;
        s_ota_anim_ms = now;
        drawOtaFull(st, ph, now);
    } else if (ph == OtaPhase::Downloading && st.ota_progress != s_ota_pct) {
        s_ota_pct = st.ota_progress;
        drawOtaPct(ph, st.ota_progress);
        drawOtaBar(ph, st.ota_progress, now);
    } else if (ph == OtaPhase::Flashing && now - s_ota_anim_ms >= OTA_ANIM_MS) {
        s_ota_anim_ms = now;
        drawOtaBar(ph, st.ota_progress, now);
    }
    if (ph == OtaPhase::Failed) {
        if (s_ota_failed_ms == 0) {
            s_ota_failed_ms = now;
        } else if (now - s_ota_failed_ms >= OTA_FAILED_HOLD_MS) {
            s_page = Page::Main;
            enterMain(st, now);
        }
    } else {
        s_ota_failed_ms = 0;
    }
}

void begin() {
    // SW I2C (bit-bang U8g2), mirroring the soil_sensor DisplayManager — the
    // field-proven config on the NodeMCU V3 OLED, where HW I2C (Wire) is not
    // reliable (see the header comment). Defaults: NodeMCU SCL=D1/5,
    // SDA=D2/4.
    const int scl = (PNEX_SCREEN_SCL >= 0) ? PNEX_SCREEN_SCL : 5;
    const int sda = (PNEX_SCREEN_SDA >= 0) ? PNEX_SCREEN_SDA : 4;
    // One boot line so a blank panel can be told apart from wrong pins.
    Serial.printf("[screen] ssd1306 sw-i2c sda=%d scl=%d\n", sda, scl);
    u8g2 = new U8G2_SSD1306_128X64_NONAME_F_SW_I2C(
        U8G2_R2, /*clock=*/ scl, /*data=*/ sda, /*reset=*/ U8X8_PIN_NONE);
    if (!u8g2) {
        return;
    }
    u8g2->begin();
    enterConnexion(pnex_status::state(), millis());
    flush();
}

void tick() {
    if (!u8g2) {
        return;
    }
    const pnex_status::NetState& st = pnex_status::state();
    const unsigned long now = millis();
    noteTraffic(st, now);
    if (st.link_ok) {
        s_ever_link = true;
    }
    switch (s_page) {
        case Page::Connexion:
            if (registeredHeld(st, now)) {
                s_page = Page::Main;
                enterMain(st, now);
            } else {
                tickConnexion(st, now);
            }
            break;
        case Page::Main:
            // Regression stays on MAIN (decision 2026-09-21): only the bar
            // reflects it (0 bars, dot crossed).
            if (otaActive(st)) {
                s_page = Page::Ota;
                enterOta();
                tickOta(st, now);
            } else {
                tickPins(st, now);
            }
            break;
        case Page::Ota:
            tickOta(st, now);
            break;
    }
    tickBar(st, now);
    flush();
}

// ────────────────────────── ST7735 TFT (SPI) ──────────────────────────

#elif PNEX_SCREEN_ST7735 == 1

static Adafruit_ST7735* tft = nullptr;

constexpr int SCREEN_W = 160;  // rotation 1 (landscape)
constexpr int SCREEN_H = 128;

// Login "gerbe de faisceaux" palette (from assets/tron-gerbe.js shader).
constexpr uint16_t COLOR_BG_TEAL = 0x0946;
constexpr uint16_t GERBE_PALETTE[] = {
    0x1BF6, 0xD169, 0xEB85, 0x81F2, 0x06B4, 0xC92E,
};
constexpr uint16_t LOGO_RED = 0xE0C9;
constexpr uint16_t LOGO_DARK = 0x1926;
constexpr uint16_t COLOR_AZURE = 0x05BF;

// V2 status tokens (validated 2026-09-21): green = server link OK + TX,
// orange = RX (same value as GERBE_PALETTE[2], named for the bar), dim =
// any idle/upcoming element, LOGO_RED doubles as "server link lost".
constexpr uint16_t dimmer(uint16_t c) {  // 25% intensity, same hue
    return (((c >> 11) & 0x1F) >> 2) << 11 | (((c >> 5) & 0x3F) >> 2) << 5 |
           ((c & 0x1F) >> 2);
}
constexpr uint16_t COLOR_GREEN = 0x4FE0;
constexpr uint16_t COLOR_ORANGE = 0xEB85;
constexpr uint16_t COLOR_DIM_AZURE = dimmer(COLOR_AZURE);

// ── Gerbe geometry (ported from tft_st7735_demo) ──

struct Segment {
    float head;
    float len;
};

struct Beam {
    float entryX;
    uint16_t color;
    Segment segs[3];
};

constexpr int BEAM_COUNT = 16;
constexpr float FOCUS_X = SCREEN_W / 2.0f;
constexpr float FOCUS_Y = -10.0f;
constexpr float FLOOR_Y = 114.0f;
constexpr float CURVE_PULL = 1.0f;
constexpr float SEG_SPEED = 0.24f;
constexpr float SEG_MIN = 0.18f;
constexpr float SEG_MAX = 0.50f;

static Beam beams[BEAM_COUNT];

static float rand01() { return random(1000) / 1000.0f; }

static void bezPoint(const Beam& b, float u, float& x, float& y) {
    const float cx = b.entryX + (FOCUS_X - b.entryX) * CURVE_PULL;
    const float cy = 52.0f;
    const float w0 = (1.0f - u) * (1.0f - u);
    const float w1 = 2.0f * (1.0f - u) * u;
    const float w2 = u * u;
    x = w0 * b.entryX + w1 * cx + w2 * FOCUS_X;
    y = w0 * FLOOR_Y + w1 * cy + w2 * FOCUS_Y;
}

// Segments glide toward the focus; one that fully exits is reborn below
// the floor with a fresh random length.
static void updateSegments() {
    static uint32_t last = 0;
    const float dt = (millis() - last) / 1000.0f;
    last = millis();
    if (dt <= 0.0f || dt > 0.5f) {
        return;  // ignore boot jump and hiccups
    }
    for (int i = 0; i < BEAM_COUNT; i++) {
        for (int s = 0; s < 3; s++) {
            Segment& sg = beams[i].segs[s];
            sg.head += SEG_SPEED * dt;
            if (sg.head - sg.len > 1.0f) {
                sg.head = -rand01() * 0.5f;
                sg.len = SEG_MIN + rand01() * (SEG_MAX - SEG_MIN);
            }
        }
    }
}

// Works against any Adafruit_GFX backend (canvas or direct display).
static void drawBeam(Adafruit_GFX& g, const Beam& b) {
    for (int s = 0; s < 3; s++) {
        const Segment& sg = b.segs[s];
        int from = (int)((sg.head - sg.len) * 120.0f);
        int to = (int)(sg.head * 120.0f);
        if (from < 0) {
            from = 0;
        }
        if (to > 120) {
            to = 120;
        }
        for (int st = from; st <= to; st++) {
            float x, y;
            bezPoint(b, st / 120.0f, x, y);
            const int yi = (int)y;
            if (yi >= 0 && yi < (int)FLOOR_Y) {
                g.drawPixel((int)x, yi, b.color);
                g.drawPixel((int)x + 1, yi, b.color);  // 2 px thick
            }
        }
    }
}

// Thick diagonal cross (5 px arms) — the coral x of the wordmark.
static void drawXCross(Adafruit_GFX& g, int x, int y, int s, uint16_t color) {
    for (int i = 0; i <= s; i++) {
        for (int w = 0; w < 5; w++) {
            g.drawPixel(x + i + w, y + i, color);
            g.drawPixel(x + s - i + w, y + i, color);
        }
    }
}

// Wordmark on its white pill, like the login card: near-black "pne" + red x.
static void drawLogo(Adafruit_GFX& g, int y) {
    const uint8_t size = 4;
    const int wText = 6 * size * 3;  // "pne" (includes the trailing cell space)
    const int gs = 20;               // x size: matches the letters' height
    const int xText = (SCREEN_W - (wText - 4) - gs) / 2;
    const int gx = xText + wText - 4, gy = y + 8;  // x pulled into that space

    // White rounded pill behind the wordmark (the login card echo).
    g.fillRoundRect(xText - 6, y - 6, (gx + gs) - xText + 16, 32 + 12, 8,
                    ST77XX_WHITE);

    g.setTextSize(size);
    g.setTextColor(LOGO_DARK);
    g.setCursor(xText, y);
    g.print("pne");
    drawXCross(g, gx, gy, gs, LOGO_RED);
    g.fillCircle(gx + gs / 2 + 3, gy + gs / 2, 3, COLOR_AZURE);  // dot
}

static void initBeams() {
    for (int i = 0; i < BEAM_COUNT; i++) {
        beams[i].entryX = SCREEN_W / 2.0f + (i - (BEAM_COUNT - 1) / 2.0f) * 10.4f;
        beams[i].color = GERBE_PALETTE[i % 6];
        for (int s = 0; s < 3; s++) {
            beams[i].segs[s].head = rand01() * 1.5f - 0.5f;
            beams[i].segs[s].len = SEG_MIN + rand01() * (SEG_MAX - SEG_MIN);
        }
    }
}

// ── V2 layout: persistent status bar + connection timeline ──
// (from pnex-tft-mockups.html at /3)

constexpr int BAR_SEP_Y = 109;  // separator line above the bar
constexpr int BAR_BASE = 124;   // bottom alignment of the bar icons
constexpr int TL_LINE_Y = 94;   // timeline track
constexpr uint8_t TL_X[3] = {20, 80, 140};  // node centers
constexpr int TL_R = 3;                    // node radius (Ø7 with ring)
constexpr int TL_LABEL_Y = 101;            // labels top line

static const char* const TL_LABELS[3] = {"WiFi", "WS", "Reg"};

// Server dot: green = link OK, red = lost (was up once), dim = not yet.
static uint16_t dotColor(uint8_t mode) {
    return mode == 1 ? COLOR_GREEN : (mode == 2 ? LOGO_RED : COLOR_DIM_AZURE);
}

// 7 px wide, 5 px tall triangle, apex toward `up` (TX ▲ / RX ▼). Widths are
// row-major (apex 1 px → base 7 px); the down case mirrors the table instead
// of reindexing it by the loop var (loop var and row used to be decoupled,
// which drew both arrows pointing the wrong way).
static void drawTri(Adafruit_GFX& g, int x, int y, bool up, uint16_t color) {
    static const uint8_t w[5] = {1, 3, 5, 5, 7};
    for (int row = 0; row < 5; row++) {
        const uint8_t width = up ? w[row] : w[4 - row];
        g.drawFastHLine(x + (7 - width) / 2, y + row, width, color);
    }
}

// 4 bars, 2 px wide, gap 1, heights 2/4/5/6, bottom-aligned (right block).
static void drawWifiBars(Adafruit_GFX& g, uint8_t level, uint16_t on_c,
                         uint16_t off_c) {
    static const uint8_t h[4] = {2, 4, 5, 6};
    g.fillRect(108, BAR_BASE - 6, 11, 6, COLOR_BG_TEAL);  // clear own zone
    int x = 108;
    for (int i = 0; i < 4; i++) {
        g.fillRect(x, BAR_BASE - h[i], 2, h[i], level > i ? on_c : off_c);
        x += 3;
    }
}

// Ø5 dot — same footprint for every state, only the color changes.
static void drawLinkDot(Adafruit_GFX& g, uint16_t color) {
    g.fillCircle(126, BAR_BASE - 2, 2, color);
}

static void drawTxRx(Adafruit_GFX& g, bool tx_on, bool rx_on) {
    drawTri(g, 136, BAR_BASE - 5, true, tx_on ? COLOR_GREEN : COLOR_DIM_AZURE);
    drawTri(g, 148, BAR_BASE - 5, false,
            rx_on ? COLOR_ORANGE : COLOR_DIM_AZURE);
}

// Separator + device id (drawn once per full-bar pass; the id never churns).
static void drawBarChrome(Adafruit_GFX& g) {
    g.drawFastHLine(0, BAR_SEP_Y, SCREEN_W, COLOR_DIM_AZURE);
    g.setTextSize(1);
    g.setTextColor(COLOR_AZURE);
    g.setCursor(5, BAR_BASE - 7);
    char buf[17];  // bar budget: 16 chars of the (up to 64) device id
    snprintf(buf, sizeof(buf), "%.16s", s_device_id);
    g.print(buf);
}

// Connection timeline: track, fill up to the reached node, 3 nodes, labels.
// Validated = full azure; current = white blinking; upcoming = dim ring.
static void drawTimeline(Adafruit_GFX& g, const pnex_status::NetState& st,
                         bool blink_on) {
    const uint8_t idx = stepIdx(st);
    const bool reg = (st.step == pnex_status::Step::Registered);
    g.drawFastHLine(TL_X[0], TL_LINE_Y, TL_X[2] - TL_X[0] + 1,
                    COLOR_DIM_AZURE);
    if (idx > 0) {
        g.drawFastHLine(TL_X[0], TL_LINE_Y, TL_X[idx] - TL_X[0], COLOR_AZURE);
    }
    g.setTextSize(1);
    for (uint8_t i = 0; i < 3; i++) {
        const int x = TL_X[i];
        uint16_t c;
        bool filled;
        if (reg || i < idx) {
            c = COLOR_AZURE;
            filled = true;
        } else if (i == idx) {
            c = blink_on ? ST77XX_WHITE : COLOR_DIM_AZURE;
            filled = blink_on;
        } else {
            c = COLOR_DIM_AZURE;
            filled = false;
        }
        g.fillCircle(x, TL_LINE_Y, TL_R, COLOR_BG_TEAL);  // beams behind
        g.drawCircle(x, TL_LINE_Y, TL_R, c);
        if (filled) {
            g.fillCircle(x, TL_LINE_Y, TL_R - 1, c);
        }
        const char* lb = TL_LABELS[i];
        g.setTextColor(c);
        g.setCursor(x - (int)(strlen(lb) * 6) / 2, TL_LABEL_Y);
        g.print(lb);
    }
}

// Bar caches — once MAIN is static, each section redraws on change only.
static uint8_t s_bar_lvl = 255, s_bar_dot = 255;
static bool s_bar_tx = false, s_bar_rx = false;

// Full bar pass (page entry / every animated connexion frame).
static void drawBarFull(Adafruit_GFX& g, const pnex_status::NetState& st,
                        bool tx_on, bool rx_on) {
    g.fillRect(0, BAR_SEP_Y, SCREEN_W, SCREEN_H - BAR_SEP_Y, COLOR_BG_TEAL);
    drawBarChrome(g);
    const uint8_t lvl = st.wifi_ok ? st.wifi_level : 0;
    const uint8_t dot = dotMode(st);
    drawWifiBars(g, lvl, COLOR_AZURE, COLOR_DIM_AZURE);
    drawLinkDot(g, dotColor(dot));
    drawTxRx(g, tx_on, rx_on);
    s_bar_lvl = lvl;
    s_bar_dot = dot;
    s_bar_tx = tx_on;
    s_bar_rx = rx_on;
}

// Incremental bar pass (static MAIN page): redraw a section only when its
// state changed. Direct draws on *tft — never a full clear.
static void tickMainBar(const pnex_status::NetState& st, unsigned long now) {
    const uint8_t lvl = st.wifi_ok ? st.wifi_level : 0;
    if (lvl != s_bar_lvl) {
        s_bar_lvl = lvl;
        drawWifiBars(*tft, lvl, COLOR_AZURE, COLOR_DIM_AZURE);
    }
    const uint8_t dot = dotMode(st);
    if (dot != s_bar_dot) {
        s_bar_dot = dot;
        drawLinkDot(*tft, dotColor(dot));
    }
    const bool tx = txOn(now), rx = rxOn(now);
    if (tx != s_bar_tx || rx != s_bar_rx) {
        s_bar_tx = tx;
        s_bar_rx = rx;
        drawTxRx(*tft, tx, rx);
    }
}

// ── MAIN pin panel (the "zone data"): live readout of the provisioned
// pins, two columns × 13 rows. Labels/modes come from the pnex_status
// snapshot (the ProvisionAck table — screen gpios already excluded
// server-side); values are sampled straight from the gpios at draw time,
// subscription-free. Redraw is change-only per row.

constexpr int PIN_COL_X[2] = {4, 84};
constexpr int PIN_ROWS = 13;              // 13 rows × 8 px = 104 ≤ BAR_SEP_Y
constexpr int PIN_COL_W = 76;             // label zone + 4-char value cell
constexpr int PIN_REFRESH_MS = 150;
constexpr int PIN_PANEL_MAX = PIN_ROWS * 2;  // 26 slots (snapshot holds 32)

// Per-slot cache for change-only redraws (gpio 255 = empty slot).
struct PinRowCache {
    uint8_t gpio = 255;
    uint8_t mode = 255;
    uint32_t value = 0;
};
static PinRowCache s_pin_rows[PIN_PANEL_MAX];
static uint8_t s_pin_total = 255;  // 255 = panel not built yet
static unsigned long s_pins_ms = 0;

// One row: label (azure) + right-aligned value (white) on the bg.
static void drawPinRow(int slot, const pnex_status::PinEntry& e, uint32_t v) {
    const int x = PIN_COL_X[slot / PIN_ROWS];
    const int y = (slot % PIN_ROWS) * 8;
    tft->fillRect(x, y, PIN_COL_W, 8, COLOR_BG_TEAL);
    tft->setTextSize(1);
    char lab[10];
    snprintf(lab, sizeof(lab), "%.8s", e.label);
    tft->setTextColor(COLOR_AZURE);
    tft->setCursor(x, y);
    tft->print(lab);
    char vb[8];
    formatPinValue(vb, sizeof(vb), e, v);
    tft->setTextColor(ST77XX_WHITE);
    tft->setCursor(x + PIN_COL_W - (int)strlen(vb) * 6, y);
    tft->print(vb);
}

// Incremental panel pass (throttled): sample + redraw changed rows only.
// A table republish with a different count wipes and rebuilds the panel.
static void tickMainPins(const pnex_status::NetState& st, unsigned long now) {
    if (now - s_pins_ms < PIN_REFRESH_MS) {
        return;
    }
    s_pins_ms = now;
    int n = st.pin_count;
    if (n > PIN_PANEL_MAX) {
        n = PIN_PANEL_MAX;
    }
    if (n != (int)s_pin_total) {
        tft->fillRect(0, 0, SCREEN_W, BAR_SEP_Y, COLOR_BG_TEAL);
        for (int i = 0; i < PIN_PANEL_MAX; i++) {
            s_pin_rows[i] = PinRowCache{};
        }
        s_pin_total = (uint8_t)n;
    }
    for (int i = 0; i < n; i++) {
        const pnex_status::PinEntry& e = st.pins[i];
        const uint32_t v = samplePin(e);
        PinRowCache& c = s_pin_rows[i];
        if (c.gpio == e.gpio && c.mode == (uint8_t)e.mode && c.value == v) {
            continue;
        }
        drawPinRow(i, e, v);
        c.gpio = e.gpio;
        c.mode = (uint8_t)e.mode;
        c.value = v;
    }
}

// MAIN page entry: clear the center once, then either the pin panel (first
// pass builds it within PIN_REFRESH_MS) or the placeholder when no pins are
// provisioned. Direct draws — the canvas is left alone from here on.
static void enterMain(const pnex_status::NetState& st, unsigned long now) {
    tft->fillRect(0, 0, SCREEN_W, BAR_SEP_Y, COLOR_BG_TEAL);
    tft->setTextSize(1);
    if (st.pin_count == 0) {
        tft->setTextColor(COLOR_AZURE);
        tft->setCursor((SCREEN_W - 13 * 6) / 2, 44);
        tft->print("- zone data -");
        tft->setTextColor(COLOR_DIM_AZURE);
        tft->setCursor((SCREEN_W - 24 * 6) / 2, 60);
        tft->print("(transmission a definir)");
        s_pin_total = 0;
    } else {
        s_pin_total = 255;  // force the first panel pass to build
    }
    drawBarFull(*tft, st, txOn(now), rxOn(now));
}

// ── OTA page: download/flash progress in the boot-timeline visual language
// (track + nodes + white blink). The network side publishes phase+progress
// into pnex_status from the BLOCKING download loop (which also ticks the
// screen every iteration); entry is level-triggered (downloading/flashing),
// failed holds a few seconds then falls back to MAIN — the device keeps the
// old firmware.

constexpr int OTA_TITLE_Y = 14;
constexpr int OTA_NUM_Y = 32;   // pct number, size 3 (24 px tall)
constexpr int OTA_TRACK_Y = 78;
constexpr int OTA_LABEL_Y = 86;
constexpr int OTA_NX[2] = {40, 120};
constexpr int OTA_NODE_R = 3;   // same footprint as the boot timeline

static unsigned long s_ota_failed_ms = 0;
static uint8_t s_ota_sig = 255;  // (phase, blink) last drawn
static uint8_t s_ota_pct = 255;  // number last drawn (254 = "failed" text)

// Page entry: clear the center, paint the static chrome (title + dim base
// track); the state pass draws nodes/labels/fill and the number.
static void enterOta() {
    tft->fillRect(0, 0, SCREEN_W, BAR_SEP_Y, COLOR_BG_TEAL);
    tft->setTextSize(1);
    tft->setTextColor(COLOR_AZURE);
    tft->setCursor((SCREEN_W - 10 * 6) / 2, OTA_TITLE_Y);
    tft->print("OTA update");
    tft->drawFastHLine(OTA_NX[0], OTA_TRACK_Y, OTA_NX[1] - OTA_NX[0],
                       COLOR_DIM_AZURE);
    s_ota_failed_ms = 0;
    s_ota_sig = 255;
    s_ota_pct = 255;
}

// One state pass: track fill, node rings, labels — redrawn on (phase,
// blink) edge only. Same grammar as the boot timeline: validated = azure,
// current = white blinking, upcoming = dim.
static void drawOtaState(const pnex_status::NetState& st, OtaPhase ph,
                         bool blink) {
    const uint16_t accent = (ph == OtaPhase::Failed) ? LOGO_RED : COLOR_AZURE;
    // Track fill: proportional while downloading, full otherwise (a safety
    // dim line covers any regression — e.g. failed right after a full fill).
    const int span = OTA_NX[1] - OTA_NX[0];
    int fill = span;
    if (ph == OtaPhase::Downloading) {
        fill = (int)(span * st.ota_progress / 100);
    }
    tft->drawFastHLine(OTA_NX[0], OTA_TRACK_Y, fill, accent);
    if (fill < span) {
        tft->drawFastHLine(OTA_NX[0] + fill, OTA_TRACK_Y, span - fill,
                           COLOR_DIM_AZURE);
    }
    // Node colors (mirrored on the labels).
    uint16_t nc[2];
    for (int i = 0; i < 2; i++) {
        if (ph == OtaPhase::Failed) {
            nc[i] = LOGO_RED;
        } else if (ph == OtaPhase::Flashing) {
            nc[i] = (i == 0) ? COLOR_AZURE : (blink ? ST77XX_WHITE : COLOR_DIM_AZURE);
        } else {
            nc[i] = (i == 0) ? (blink ? ST77XX_WHITE : COLOR_DIM_AZURE) : COLOR_DIM_AZURE;
        }
    }
    for (int i = 0; i < 2; i++) {
        const int x = OTA_NX[i];
        const bool filled =
            (ph == OtaPhase::Failed) || (i == 0 && ph == OtaPhase::Flashing) ||
            (blink && i == 0 && ph == OtaPhase::Downloading);
        tft->drawCircle(x, OTA_TRACK_Y, OTA_NODE_R, nc[i]);
        if (filled) {
            tft->fillCircle(x, OTA_TRACK_Y, OTA_NODE_R - 1, nc[i]);
        } else {
            tft->fillCircle(x, OTA_TRACK_Y, OTA_NODE_R - 1, COLOR_BG_TEAL);
        }
    }
    static const char* const LB[2] = {"Download", "Flash"};
    tft->setTextSize(1);
    for (int i = 0; i < 2; i++) {
        tft->setTextColor(nc[i]);
        tft->setCursor(OTA_NX[i] - (int)(strlen(LB[i]) * 6) / 2, OTA_LABEL_Y);
        tft->print(LB[i]);
    }
}

// Center readout: big pct (size 3, white) — or "failed" in red (size 2).
static void drawOtaNumber(OtaPhase ph, uint8_t pct) {
    tft->fillRect(0, OTA_NUM_Y - 2, SCREEN_W, 30, COLOR_BG_TEAL);
    char buf[8];
    int cw;
    if (ph == OtaPhase::Failed) {
        tft->setTextSize(2);
        tft->setTextColor(LOGO_RED);
        snprintf(buf, sizeof(buf), "failed");
        cw = 12;
    } else {
        tft->setTextSize(3);
        tft->setTextColor(ST77XX_WHITE);
        snprintf(buf, sizeof(buf), "%u%%", pct);
        cw = 18;
    }
    tft->setCursor((SCREEN_W - (int)strlen(buf) * cw) / 2, OTA_NUM_Y);
    tft->print(buf);
}

// OTA page pass: state redraw on (phase, blink) edge, number on change,
// failed held OTA_FAILED_HOLD_MS then fallback to MAIN.
static void tickOtaPage(const pnex_status::NetState& st, unsigned long now) {
    const OtaPhase ph = otaPhaseIdx(st);
    const bool blink = blinkOn(now);
    const uint8_t sig = (uint8_t)(((uint8_t)ph << 1) | (blink ? 1 : 0));
    if (sig != s_ota_sig) {
        s_ota_sig = sig;
        drawOtaState(st, ph, blink);
    }
    const uint8_t shown = (ph == OtaPhase::Failed) ? 254 : st.ota_progress;
    if (shown != s_ota_pct) {
        s_ota_pct = shown;
        drawOtaNumber(ph, st.ota_progress);
    }
    if (ph == OtaPhase::Failed) {
        if (s_ota_failed_ms == 0) {
            s_ota_failed_ms = now;
        } else if (now - s_ota_failed_ms >= OTA_FAILED_HOLD_MS) {
            s_page = Page::Main;
            enterMain(st, now);
        }
    } else {
        s_ota_failed_ms = 0;
    }
}

#if !defined(ESP8266)
// ── Full-quality path (ESP32 / C3 / S3): canvas gerbe, animated ──

static GFXcanvas16* cv = nullptr;   // 40 KB heap — fine on ESP32 family
static unsigned long s_last_frame = 0;

// Floor reflection: mirror the rows above FLOOR_Y, dimmed.
static void drawReflection(GFXcanvas16& g) {
    for (int dy = 1; dy <= 14; dy++) {
        const int srcY = (int)FLOOR_Y - dy, dstY = (int)FLOOR_Y + dy;
        if (srcY < 0 || dstY >= SCREEN_H) {
            continue;
        }
        for (int x = 0; x < SCREEN_W; x++) {
            const uint16_t px = g.getPixel(x, srcY);
            if (px != COLOR_BG_TEAL) {
                g.drawPixel(x, dstY, dimmer(px));
            }
        }
    }
}

// One connexion frame: gerbe + logo, timeline and bar painted on top (the
// canvas is pushed whole anyway — no extra full-clear for the overlays).
static void splashFrame(const pnex_status::NetState& st, bool blink_on,
                        bool tx_on, bool rx_on) {
    updateSegments();
    cv->fillScreen(COLOR_BG_TEAL);
    for (int i = 0; i < BEAM_COUNT; i++) {
        drawBeam(*cv, beams[i]);
    }
    drawReflection(*cv);
    if (millis() > 600) {
        drawLogo(*cv, 48);
    }
    drawTimeline(*cv, st, blink_on);
    drawBarFull(*cv, st, tx_on, rx_on);
    tft->drawRGBBitmap(0, 0, cv->getBuffer(), SCREEN_W, SCREEN_H);
}

static void tftBegin() {
    tft = new Adafruit_ST7735(PNEX_SCREEN_CS, PNEX_SCREEN_DC, PNEX_SCREEN_RST);
    SPI.begin(PNEX_SCREEN_SCK, -1, PNEX_SCREEN_MOSI, PNEX_SCREEN_CS);
    tft->initR(INITR_BLACKTAB);
    tft->setRotation(1);       // landscape: 160x128
    tft->setSPISpeed(8000000); // stable with sustained full-frame pushes

    cv = new GFXcanvas16(SCREEN_W, SCREEN_H);
    if (cv->getBuffer() == nullptr) {  // OOM fallback: static wordmark
        tft->fillScreen(ST77XX_BLACK);
        tft->setTextSize(4);
        tft->setTextColor(ST77XX_WHITE);
        tft->setCursor(30, 48);
        tft->print("pnex");
    }
    initBeams();
}

static void pathTickConnexion(const pnex_status::NetState& st,
                              unsigned long now) {
    if (now - s_last_frame < 20) {
        return;  // ~50 fps while connecting
    }
    const bool blink = blinkOn(now);
    splashFrame(st, blink, txOn(now), rxOn(now));
    s_last_frame = now;
}

#else  // ESP8266
// ── Degraded path (ESP8266): one static frame, no canvas, no animation.

// Sparse static beams (line strips, not per-pixel) for the boot splash.
static void drawStaticGerbe() {
    tft->fillScreen(COLOR_BG_TEAL);
    for (int i = 0; i < BEAM_COUNT; i++) {
        const Beam& b = beams[i];
        for (int st = 0; st < 120; st += 4) {
            float x0, y0, x1, y1;
            bezPoint(b, st / 120.0f, x0, y0);
            bezPoint(b, (st + 4) / 120.0f, x1, y1);
            if (y0 >= 0 && y0 < FLOOR_Y) {
                tft->drawLine((int)x0, (int)y0, (int)x1, (int)y1, b.color);
            }
        }
    }
    drawLogo(*tft, 48);
}

static void tftBegin() {
    tft = new Adafruit_ST7735(PNEX_SCREEN_CS, PNEX_SCREEN_DC, PNEX_SCREEN_RST);
    SPI.begin();  // HSPI pinned: SCK=14, MISO=12, MOSI=13
    tft->initR(INITR_BLACKTAB);
    tft->setRotation(1);
    tft->setSPISpeed(8000000);
    initBeams();
    drawStaticGerbe();
}

static uint8_t s_tl_sig = 255;  // last drawn timeline state (incl. blink)

static void pathTickConnexion(const pnex_status::NetState& st,
                              unsigned long now) {
    const bool blink = blinkOn(now);
    const uint8_t sig = (uint8_t)(stepIdx(st) | (blink ? 2 : 0));
    if (sig != s_tl_sig) {
        s_tl_sig = sig;
        // Repaint the timeline band (clears the beams behind it — degraded
        // path, clean band assumed).
        tft->fillRect(0, TL_LINE_Y - 6, SCREEN_W, 20, COLOR_BG_TEAL);
        drawTimeline(*tft, st, blink);
    }
    tickMainBar(st, now);  // the bar stays incremental
}

#endif  // !ESP8266 / ESP8266

static bool s_started = false;

void begin() {
    s_started = true;
    tftBegin();
}

void tick() {
    if (!s_started) {
        return;
    }
    const pnex_status::NetState& st = pnex_status::state();
    const unsigned long now = millis();
    noteTraffic(st, now);
    if (st.link_ok) {
        s_ever_link = true;
    }
    if (s_page == Page::Main) {
        // Regression stays on MAIN (decision 2026-09-21): only the bar
        // reflects it (0 bars, dot red).
        tickMainBar(st, now);
        tickMainPins(st, now);
        // An OTA download starting takes over the center zone (level
        // trigger: downloading/flashing; failed alone never re-enters).
        if (otaActive(st)) {
            s_page = Page::Ota;
            enterOta();
        }
        return;
    }
    if (s_page == Page::Ota) {
        tickOtaPage(st, now);
        tickMainBar(st, now);
        return;
    }
    if (registeredHeld(st, now)) {
        s_page = Page::Main;
        enterMain(st, now);
        return;
    }
    pathTickConnexion(st, now);
}

// ────────────────────────── No screen compiled ──────────────────────────

#else

void begin() {}

void tick() {}

#endif

}  // namespace pnex_screen
