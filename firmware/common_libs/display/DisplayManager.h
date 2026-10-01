//
// Created by shan on 15/05/24.
//

#ifndef DISPLAYMANAGER_H
#define DISPLAYMANAGER_H

#include <U8g2lib.h>

class DisplayManager {
public:
    DisplayManager();
    void init();
    void showMessage(const char* message);
    void showDeviceID(const char* deviceID);
    static void add_diagonal_bar(unsigned char* icon, int width, int height);
    void wifiConnected();
    void wifiDisconnected();
    void webSocketConnected();
    void webSocketDisconnected();
    void showArrowUp() ;
    void hideArrowUp();
    void showArrowDown();
    void hideArrowDown();
    void show4RelayStatus(bool relay1, bool relay2, bool relay3, bool relay4);
    void show4RelayStatusWithHysteresis(bool relay1, bool relay2, bool relay3, bool relay4,
                                         bool hyst1, bool hyst2, bool hyst3, bool hyst4);
    void showValue(const char* label, float value, const char* unit, int x, int y);
    void clear();
    void showLoadingAnimation(const char* title, int durationMs, int updateIntervalMs);
    void showLoadingProgress(const char* title, int progress, const char* statusText);
    void showLoadingProgressAnimated(const char* title, int fromProgress, int toProgress, const char* statusText, int animationMs);

private:
    U8G2_SSD1306_128X64_NONAME_F_SW_I2C u8g2;
};

#endif // DISPLAYMANAGER_H
