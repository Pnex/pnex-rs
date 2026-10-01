// pnex boot splash on a 1.77" SPI TFT (ST7735, 160x128) + ESP32-WROOM.
//
// Wiring (standard ESP32 devkit, VSPI):
//   TFT VCC -> 3V3      TFT GND -> GND      TFT LEDA -> 3V3
//   TFT SCK -> GPIO18   TFT SDA -> GPIO23   TFT CS   -> GPIO5
//   TFT DC  -> GPIO2    TFT RST -> GPIO4
//
// Splash-only mode: the login "gerbe de faisceaux" animation
// (assets/tron-gerbe.js, simplified) loops forever — a dense near-straight
// sheaf of colored beams entering along the full bottom edge, converging
// into a teal glow at the top, with a dim mirrored reflection below the
// floor line. pnex wordmark on top. A serial heartbeat (1 Hz) confirms the
// board is alive.
//
// Cheap ST7735 panels vary: if colors look wrong (red/blue swapped) or the
// picture is shifted by a few pixels, change TFT_TAB below (INITR_BLACKTAB,
// INITR_REDTAB, INITR_GREENTAB, INITR_GREENTAB160x80).

#include <Adafruit_GFX.h>
#include <Adafruit_ST7735.h>
#include <SPI.h>

constexpr int TFT_CS = 5;
constexpr int TFT_DC = 2;   // strapping pin: the TFT input is Hi-Z at reset, safe to share
constexpr int TFT_RST = 4;
constexpr uint8_t TFT_TAB = INITR_BLACKTAB;

constexpr int SCREEN_W = 160;  // rotation 1 (landscape)
constexpr int SCREEN_H = 128;

// Login "gerbe de faisceaux" palette (from assets/tron-gerbe.js shader)
constexpr uint16_t COLOR_BG_TEAL = 0x0946;     // shader background
constexpr uint16_t GERBE_PALETTE[] = {
    0x1BF6, // teal
    0xD169, // red
    0xEB85, // orange
    0x81F2, // purple
    0x06B4, // indigo
    0xC92E, // magenta
};

// Coral red of the logo "x" glyph
constexpr uint16_t LOGO_RED = 0xE0C9;
constexpr uint16_t LOGO_DARK = 0x1926;         // near-black navy of the wordmark
constexpr uint16_t COLOR_AZURE = 0x05BF;       // blue dot at the x center

Adafruit_ST7735 tft(TFT_CS, TFT_DC, TFT_RST);

// --- gerbe splash -----------------------------------------------------------

constexpr uint16_t dimmer(uint16_t c) {  // 25% intensity, same hue
    return (((c >> 11) & 0x1F) >> 2) << 11 | (((c >> 5) & 0x3F) >> 2) << 5 |
           ((c & 0x1F) >> 2);
}

struct Segment {
    float head;              // leading edge along the beam (u: 0 floor -> 1 focus)
    float len;               // segment length in u
};

struct Beam {
    float entryX;            // entry on the floor line, spread over the full width
    uint16_t color;
    Segment segs[3];         // lit portions gliding toward the focus
};

constexpr int BEAM_COUNT = 16;
constexpr float FOCUS_X = SCREEN_W / 2.0f;  // convergence above the screen
constexpr float FOCUS_Y = -10.0f;
constexpr float FLOOR_Y = 114.0f;  // beams spring from here; below: reflection
constexpr float CURVE_PULL = 1.0f;    // control on the focus axis: vertical at
                                      // the neck, flaring toward the floor
constexpr float SEG_SPEED = 0.24f;    // u per second, shared by every segment
constexpr float SEG_MIN = 0.18f;      // segment length range (in u)
constexpr float SEG_MAX = 0.50f;

Beam beams[BEAM_COUNT];
GFXcanvas16 cv(SCREEN_W, SCREEN_H);  // 40 KB frame buffer

// Quadratic bezier point along a beam: floor (wide entry) -> focus (top).
void bezPoint(const Beam &b, float u, float &x, float &y) {
    const float cx = b.entryX + (FOCUS_X - b.entryX) * CURVE_PULL;
    const float cy = 52.0f;
    float w0 = (1.0f - u) * (1.0f - u);
    float w1 = 2.0f * (1.0f - u) * u;
    float w2 = u * u;
    x = w0 * b.entryX + w1 * cx + w2 * FOCUS_X;
    y = w0 * FLOOR_Y + w1 * cy + w2 * FOCUS_Y;
}

float rand01() { return random(1000) / 1000.0f; }

// Segments glide toward the focus; one that fully exits is reborn below the
// floor with a fresh random length — a "life" from birth to the next dark
// zone, no twinkle.
void updateSegments() {
    static uint32_t last = millis();
    float dt = (millis() - last) / 1000.0f;
    last = millis();
    if (dt <= 0.0f || dt > 0.5f) {
        return;  // ignore boot jump and hiccups
    }
    for (int i = 0; i < BEAM_COUNT; i++) {
        for (int s = 0; s < 3; s++) {
            Segment &sg = beams[i].segs[s];
            sg.head += SEG_SPEED * dt;
            if (sg.head - sg.len > 1.0f) {
                sg.head = -rand01() * 0.5f;
                sg.len = SEG_MIN + rand01() * (SEG_MAX - SEG_MIN);
            }
        }
    }
}

void drawBeam(GFXcanvas16 &g, const Beam &b) {
    for (int s = 0; s < 3; s++) {
        const Segment &sg = b.segs[s];
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
            int yi = (int)y;
            if (yi >= 0 && yi < (int)FLOOR_Y) {
                g.drawPixel((int)x, yi, b.color);
                g.drawPixel((int)x + 1, yi, b.color);  // 2 px thick
            }
        }
    }
}

// Floor reflection: mirror the rows above FLOOR_Y, dimmed.
void drawReflection(GFXcanvas16 &g) {
    for (int dy = 1; dy <= 14; dy++) {
        int srcY = (int)FLOOR_Y - dy, dstY = (int)FLOOR_Y + dy;
        if (srcY < 0 || dstY >= SCREEN_H) {
            continue;
        }
        for (int x = 0; x < SCREEN_W; x++) {
            uint16_t px = g.getPixel(x, srcY);
            if (px != COLOR_BG_TEAL) {
                g.drawPixel(x, dstY, dimmer(px));
            }
        }
    }
}

// Thick diagonal cross (5 px arms).
void drawXCross(GFXcanvas16 &g, int x, int y, int s, uint16_t color) {
    for (int i = 0; i <= s; i++) {
        for (int w = 0; w < 5; w++) {
            g.drawPixel(x + i + w, y + i, color);
            g.drawPixel(x + s - i + w, y + i, color);
        }
    }
}

// Wordmark on its white pill, like the login card: near-black "pne" + red x.
void drawLogo(GFXcanvas16 &g, int y) {
    const uint8_t size = 4;
    const int wText = 6 * size * 3;  // "pne" (includes the last cell's trailing space)
    const int gs = 20;               // x size: matches the letters' height
    const int xText = (SCREEN_W - (wText - 4) - gs) / 2;
    const int gx = xText + wText - 4, gy = y + 8;  // x pulled into that trailing space

    // White rounded pill behind the wordmark (the login card echo); +4 px on
    // the right, the x arms overshoot there.
    g.fillRoundRect(xText - 6, y - 6, (gx + gs) - xText + 16, 32 + 12, 8,
                    ST77XX_WHITE);

    g.setTextSize(size);
    g.setTextColor(LOGO_DARK);
    g.setCursor(xText, y);
    g.print("pne");
    drawXCross(g, gx, gy, gs, LOGO_RED);
    g.fillCircle(gx + gs / 2 + 3, gy + gs / 2, 3, COLOR_AZURE);  // dot, offset right
}

void splashFrame() {
    updateSegments();
    cv.fillScreen(COLOR_BG_TEAL);

    for (int i = 0; i < BEAM_COUNT; i++) {
        drawBeam(cv, beams[i]);
    }
    drawReflection(cv);

    if (millis() > 600) {
        drawLogo(cv, 48);
    }

    tft.drawRGBBitmap(0, 0, cv.getBuffer(), SCREEN_W, SCREEN_H);
}

void setup() {
    Serial.begin(115200);

    for (int i = 0; i < BEAM_COUNT; i++) {
        beams[i].entryX = SCREEN_W / 2.0f + (i - (BEAM_COUNT - 1) / 2.0f) * 10.4f;
        beams[i].color = GERBE_PALETTE[i % 6];
        for (int s = 0; s < 3; s++) {
            beams[i].segs[s].head = rand01() * 1.5f - 0.5f;
            beams[i].segs[s].len = SEG_MIN + rand01() * (SEG_MAX - SEG_MIN);
        }
    }

    tft.initR(TFT_TAB);
    tft.setRotation(1);  // landscape: 160x128
    tft.setSPISpeed(8000000);  // stable with the sustained full-frame pushes

    if (cv.getBuffer() == nullptr) {  // OOM fallback: static wordmark
        tft.fillScreen(ST77XX_BLACK);
        tft.setTextSize(4);
        tft.setTextColor(ST77XX_WHITE);
        tft.setCursor(30, 48);
        tft.print("pnex");
        return;
    }

    splashFrame();
    Serial.println("TFT demo ready");
}

void loop() {
    splashFrame();
    delay(20);  // pace frames

    static uint32_t lastSerial = 0;
    if (millis() - lastSerial >= 1000) {
        lastSerial = millis();
        Serial.printf("alive  heap=%lu KB  up=%lus\n",
                      (unsigned long)(ESP.getFreeHeap() / 1024),
                      (unsigned long)(millis() / 1000));
    }
}
