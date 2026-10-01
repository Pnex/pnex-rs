//
// Created by shan on 15/05/24.
//


#include "DisplayManager.h"
#include "icon_set.h"



DisplayManager::DisplayManager()
    : u8g2(U8G2_R2, /*clock*/ 14, /*data*/ 12, U8X8_PIN_NONE) {
}

void DisplayManager::init() {
  u8g2.begin();
}

void DisplayManager::showMessage(const char* message) {
  const int startX = 0;
  const int startY = 10;
  const int width = 128; // Width of the display
  const int height = 40; // Height of the display for the message region

  // Set draw color to black and clear the specific area for the message
  u8g2.setDrawColor(0);
  u8g2.drawBox(startX, startY, width, height);
  u8g2.setDrawColor(1); // Set draw color back to white

  u8g2.setFont(u8g2_font_ncenB08_tr);
  u8g2.drawStr(0, 30, message);
  u8g2.sendBuffer();
}

void DisplayManager::clear() {
  u8g2.clearBuffer();
  u8g2.sendBuffer();
}

void DisplayManager::showDeviceID(const char* deviceID) {
  // Create a buffer to hold the final message
  char message[80];
  snprintf(message, sizeof(message), "id: %s", deviceID);

  // Display the formatted message with a smaller font
  u8g2.setFont(u8g2_font_6x10_tr); // Use a smaller font, for example, 6x10

  // Calculate the position to start the text on the last line
  int y = 63; // Adjust based on your display height and font size

  // Display the message
  u8g2.setDrawColor(1); // Set draw color to white
  u8g2.drawStr(0, y, message);
  u8g2.sendBuffer();
}

void DisplayManager::add_diagonal_bar(unsigned char* icon, int width, int height) {
  for (int i = 0; i < height; i++) {
    int bit_pos = width - i - 1;
    icon[i * (width / 8) + (bit_pos / 8)] |= (1 << (7 - (bit_pos % 8)));
  }
}

// Display Wi-Fi logo based on connection status
void DisplayManager::wifiConnected() {
  u8g2.setDrawColor(1); // Set draw color to white
  u8g2.drawBitmap(0, 0, 2, 16, wifi1_icon16x16);
  u8g2.sendBuffer();
}

void DisplayManager::wifiDisconnected() {
  // Make a copy of the original icon to avoid modifying the original
  unsigned char wifi_icon_with_bar[sizeof(wifi1_icon16x16)];
  memcpy(wifi_icon_with_bar, wifi1_icon16x16, sizeof(wifi1_icon16x16));

  // Add a diagonal bar to indicate disconnection
  add_diagonal_bar(wifi_icon_with_bar, 16, 16);

  u8g2.setDrawColor(1); // Set draw color to white
  u8g2.drawBitmap(0, 0, 2, 16, wifi_icon_with_bar);
  u8g2.sendBuffer();
}

void DisplayManager::webSocketConnected() {
  u8g2.setDrawColor(1); // Set draw color to white
  // Draw WebSocket icon with 6 pixels space after Wi-Fi icon
  u8g2.drawBitmap(18, 0, 2, 16, plug_icon16x16);
  u8g2.sendBuffer();
}

void DisplayManager::webSocketDisconnected() {
  // Make a copy of the original icon to avoid modifying the original
  unsigned char plug_icon_with_bar[sizeof(plug_icon16x16)];
  memcpy(plug_icon_with_bar, plug_icon16x16, sizeof(plug_icon16x16));

  // Add a diagonal bar to indicate disconnection
  add_diagonal_bar(plug_icon_with_bar, 16, 16);

  u8g2.setDrawColor(1); // Set draw color to white
  // Draw WebSocket icon with 6 pixels space after Wi-Fi icon
  u8g2.drawBitmap(18, 0, 2, 16, plug_icon_with_bar);
  u8g2.sendBuffer();
}


// Display arrow up icon
void DisplayManager::showArrowUp() {
  u8g2.setDrawColor(1); // Set draw color to white
  // Draw arrow up icon with space after WebSocket icon
  u8g2.drawBitmap(36, 0, 2, 16, arrow_up_icon16x16);
  u8g2.sendBuffer();
}

// Hide arrow up icon
void DisplayManager::hideArrowUp() {
  u8g2.setDrawColor(0); // Set draw color to black
  // Clear the area where arrow up icon was drawn with extra padding
  u8g2.drawBox(36, 0, 18, 16);  // Increased width from 16 to 18 for safety margin
  u8g2.setDrawColor(1); // Reset draw color to white
  u8g2.sendBuffer();
}

// Display arrow down icon
void DisplayManager::showArrowDown() {
  u8g2.setDrawColor(1); // Set draw color to white
  // Draw arrow down icon with space after WebSocket icon
  u8g2.drawBitmap(54, 0, 2, 16, arrow_down_icon16x16);
  u8g2.sendBuffer();
}

// Hide arrow down icon
void DisplayManager::hideArrowDown() {
  u8g2.setDrawColor(0); // Set draw color to black
  // Clear the area where arrow down icon was drawn with extra padding
  u8g2.drawBox(54, 0, 18, 16);  // Increased width from 16 to 18 for safety margin
  u8g2.setDrawColor(1); // Reset draw color to white
  u8g2.sendBuffer();
}

// Method to display relay status
void DisplayManager::show4RelayStatus(bool relay1, bool relay2, bool relay3, bool relay4) {
  const int iconWidth = 16;  // Assuming each icon is 16x16 pixels
  const int iconHeight = 16;
  const int startX = 0;  // Start X position on the OLED display
  const int startY = 18;  // Start Y position on the second line
  const int spacing = 18;  // Horizontal spacing between icons

  // Clear the relay status area before drawing
  u8g2.setDrawColor(0);  // Set draw color to black
  u8g2.drawBox(startX, startY, 128, iconHeight);  // Clear the entire relay row
  u8g2.setDrawColor(1);  // Set draw color back to white

  unsigned char* relayIcons[4] = {
      bulb_off_icon16x16,
      bulb_off_icon16x16,
      bulb_off_icon16x16,
      bulb_off_icon16x16
  };

  // Update icon array based on the relay states
  if (relay1) relayIcons[0] = bulb_on_icon16x16;
  if (relay2) relayIcons[1] = bulb_on_icon16x16;
  if (relay3) relayIcons[2] = bulb_on_icon16x16;
  if (relay4) relayIcons[3] = bulb_on_icon16x16;

  // Draw each relay icon horizontally on the second line
  for (int i = 0; i < 4; i++) {
    u8g2.drawBitmap(startX + i * spacing, startY, 2, iconWidth, relayIcons[i]);
  }
  u8g2.sendBuffer();
}

// Method to display relay status with hysteresis indicators (hourglasses)
void DisplayManager::show4RelayStatusWithHysteresis(bool relay1, bool relay2, bool relay3, bool relay4,
                                                     bool hyst1, bool hyst2, bool hyst3, bool hyst4) {
  const int iconWidth = 16;  // Assuming each icon is 16x16 pixels
  const int iconHeight = 16;
  const int startX = 0;  // Start X position on the OLED display
  const int startY = 18;  // Start Y position on the second line
  const int hourglassY = 30;  // Y position for clock icons (moved up to avoid yellow zone)
  const int spacing = 18;  // Horizontal spacing between icons

  // Clear arrow areas to prevent overlap artifacts
  u8g2.setDrawColor(0);  // Set draw color to black
  u8g2.drawBox(36, 0, 18, 16);  // Clear arrow up area
  u8g2.drawBox(54, 0, 18, 16);  // Clear arrow down area

  // Clear the relay status area before drawing (including clock icon area)
  u8g2.drawBox(startX, startY, 128, 28);  // Clear from Y=18 to Y=46 (relay + clock rows)
  u8g2.setDrawColor(1);  // Set draw color back to white

  unsigned char* relayIcons[4] = {
      bulb_off_icon16x16,
      bulb_off_icon16x16,
      bulb_off_icon16x16,
      bulb_off_icon16x16
  };

  // Update icon array based on the relay states
  if (relay1) relayIcons[0] = bulb_on_icon16x16;
  if (relay2) relayIcons[1] = bulb_on_icon16x16;
  if (relay3) relayIcons[2] = bulb_on_icon16x16;
  if (relay4) relayIcons[3] = bulb_on_icon16x16;

  // Draw each relay icon horizontally on the second line
  for (int i = 0; i < 4; i++) {
    u8g2.drawBitmap(startX + i * spacing, startY, 2, iconWidth, relayIcons[i]);
  }

  // Draw clock icons for channels with active hysteresis (more compact than hourglass)
  bool hysteresis[4] = {hyst1, hyst2, hyst3, hyst4};
  for (int i = 0; i < 4; i++) {
    if (hysteresis[i]) {
      u8g2.drawBitmap(startX + i * spacing, hourglassY, 2, iconWidth, clock_icon16x16);
    }
  }

  u8g2.sendBuffer();
}

// Method to display any float value with a label and unit
void DisplayManager::showValue(const char* label, float value, const char* unit, int x, int y) {
  char buffer[20];  // Buffer to hold the final formatted string

  // Prepare the string
  // The format "%.2f" ensures the float is displayed with two decimal places
  snprintf(buffer, sizeof(buffer), "%s: %.2f %s", label, value, unit);

  // Set font and color for the display
  u8g2.setFont(u8g2_font_ncenB08_tr); // Choose a suitable font
  u8g2.setDrawColor(1); // White color on black background

  // Clear the area where the value will be displayed to avoid overlap (optional)
  u8g2.setDrawColor(0); // Set draw color to black
  u8g2.drawBox(x, y - 10, 128, 12); // Adjust width and height according to your font and layout
  u8g2.setDrawColor(1); // Reset draw color to white

  // Draw the string on the display
  u8g2.drawStr(x, y, buffer);
  u8g2.sendBuffer(); // Update the display with new data
}

// Loading animation with title and progress bar
void DisplayManager::showLoadingAnimation(const char* title, int durationMs, int updateIntervalMs) {
  const int progressBarY = 50;  // Y position in yellow zone (48-64)
  const int progressBarHeight = 8;
  const int progressBarWidth = 120;
  const int progressBarX = 4;

  unsigned long startTime = millis();
  unsigned long elapsed = 0;

  while (elapsed < durationMs) {
    elapsed = millis() - startTime;

    // Calculate progress percentage
    int progress = (elapsed * 100) / durationMs;
    if (progress > 100) progress = 100;

    // Calculate progress bar fill width
    int fillWidth = (progressBarWidth * progress) / 100;

    // Clear buffer
    u8g2.clearBuffer();

    // Draw title at the top
    u8g2.setFont(u8g2_font_ncenB10_tr);
    int titleWidth = u8g2.getStrWidth(title);
    int titleX = (128 - titleWidth) / 2;  // Center the title
    u8g2.drawStr(titleX, 20, title);

    // Draw "Loading" text
    u8g2.setFont(u8g2_font_ncenB08_tr);
    const char* loadingText = "Loading...";
    int loadingWidth = u8g2.getStrWidth(loadingText);
    int loadingX = (128 - loadingWidth) / 2;
    u8g2.drawStr(loadingX, 35, loadingText);

    // Draw progress bar border (in yellow zone)
    u8g2.drawFrame(progressBarX, progressBarY, progressBarWidth, progressBarHeight);

    // Draw progress bar fill
    if (fillWidth > 0) {
      u8g2.drawBox(progressBarX + 1, progressBarY + 1, fillWidth - 2, progressBarHeight - 2);
    }

    // Draw percentage text
    char percentText[8];
    snprintf(percentText, sizeof(percentText), "%d%%", progress);
    u8g2.setFont(u8g2_font_6x10_tr);
    int percentWidth = u8g2.getStrWidth(percentText);
    int percentX = (128 - percentWidth) / 2;
    u8g2.drawStr(percentX, progressBarY - 3, percentText);

    u8g2.sendBuffer();

    delay(updateIntervalMs);
  }

  // Show 100% complete for a moment
  u8g2.clearBuffer();
  u8g2.setFont(u8g2_font_ncenB10_tr);
  int titleWidth = u8g2.getStrWidth(title);
  int titleX = (128 - titleWidth) / 2;
  u8g2.drawStr(titleX, 20, title);

  u8g2.setFont(u8g2_font_ncenB08_tr);
  const char* completeText = "Complete!";
  int completeWidth = u8g2.getStrWidth(completeText);
  int completeX = (128 - completeWidth) / 2;
  u8g2.drawStr(completeX, 35, completeText);

  // Full progress bar
  u8g2.drawFrame(progressBarX, progressBarY, progressBarWidth, progressBarHeight);
  u8g2.drawBox(progressBarX + 1, progressBarY + 1, progressBarWidth - 2, progressBarHeight - 2);

  // 100% text
  u8g2.setFont(u8g2_font_6x10_tr);
  const char* hundredText = "100%";
  int hundredWidth = u8g2.getStrWidth(hundredText);
  int hundredX = (128 - hundredWidth) / 2;
  u8g2.drawStr(hundredX, progressBarY - 3, hundredText);

  u8g2.sendBuffer();
  delay(500);
}

// Non-blocking loading progress display with manual progress control
void DisplayManager::showLoadingProgress(const char* title, int progress, const char* statusText) {
  const int progressBarY = 50;  // Y position in yellow zone (48-64)
  const int progressBarHeight = 8;
  const int progressBarWidth = 120;
  const int progressBarX = 4;

  // Clamp progress to 0-100
  if (progress < 0) progress = 0;
  if (progress > 100) progress = 100;

  // Calculate progress bar fill width
  int fillWidth = (progressBarWidth * progress) / 100;

  // Clear buffer
  u8g2.clearBuffer();

  // Draw title at the top
  u8g2.setFont(u8g2_font_ncenB10_tr);
  int titleWidth = u8g2.getStrWidth(title);
  int titleX = (128 - titleWidth) / 2;  // Center the title
  u8g2.drawStr(titleX, 20, title);

  // Draw status text
  u8g2.setFont(u8g2_font_ncenB08_tr);
  int statusWidth = u8g2.getStrWidth(statusText);
  int statusX = (128 - statusWidth) / 2;
  u8g2.drawStr(statusX, 35, statusText);

  // Draw progress bar border (in yellow zone)
  u8g2.drawFrame(progressBarX, progressBarY, progressBarWidth, progressBarHeight);

  // Draw progress bar fill
  if (fillWidth > 0) {
    u8g2.drawBox(progressBarX + 1, progressBarY + 1, fillWidth - 2, progressBarHeight - 2);
  }

  // Draw percentage text
  char percentText[8];
  snprintf(percentText, sizeof(percentText), "%d%%", progress);
  u8g2.setFont(u8g2_font_6x10_tr);
  int percentWidth = u8g2.getStrWidth(percentText);
  int percentX = (128 - percentWidth) / 2;
  u8g2.drawStr(percentX, progressBarY - 3, percentText);

  u8g2.sendBuffer();
}

// Animated loading progress display with smooth transition between progress values
void DisplayManager::showLoadingProgressAnimated(const char* title, int fromProgress, int toProgress,
                                                   const char* statusText, int animationMs) {
  const int progressBarY = 50;  // Y position in yellow zone (48-64)
  const int progressBarHeight = 8;
  const int progressBarWidth = 120;
  const int progressBarX = 4;

  // Clamp progress values to 0-100
  if (fromProgress < 0) fromProgress = 0;
  if (fromProgress > 100) fromProgress = 100;
  if (toProgress < 0) toProgress = 0;
  if (toProgress > 100) toProgress = 100;

  // Calculate steps for smooth animation
  int progressDiff = toProgress - fromProgress;
  if (progressDiff == 0) {
    // No animation needed, just show final state
    showLoadingProgress(title, toProgress, statusText);
    return;
  }

  // Use smaller steps for smoother animation (aim for ~30fps)
  int frameDelay = 30;  // ~33ms per frame for 30fps
  int totalFrames = animationMs / frameDelay;
  if (totalFrames < 1) totalFrames = 1;

  unsigned long startTime = millis();
  unsigned long elapsed = 0;

  while (elapsed < animationMs) {
    elapsed = millis() - startTime;

    // Calculate current progress using linear interpolation
    float t = (float)elapsed / (float)animationMs;
    if (t > 1.0f) t = 1.0f;

    int currentProgress = fromProgress + (int)(progressDiff * t);

    // Calculate progress bar fill width
    int fillWidth = (progressBarWidth * currentProgress) / 100;

    // Clear buffer
    u8g2.clearBuffer();

    // Draw title at the top
    u8g2.setFont(u8g2_font_ncenB10_tr);
    int titleWidth = u8g2.getStrWidth(title);
    int titleX = (128 - titleWidth) / 2;  // Center the title
    u8g2.drawStr(titleX, 20, title);

    // Draw status text
    u8g2.setFont(u8g2_font_ncenB08_tr);
    int statusWidth = u8g2.getStrWidth(statusText);
    int statusX = (128 - statusWidth) / 2;
    u8g2.drawStr(statusX, 35, statusText);

    // Draw progress bar border (in yellow zone)
    u8g2.drawFrame(progressBarX, progressBarY, progressBarWidth, progressBarHeight);

    // Draw progress bar fill
    if (fillWidth > 0) {
      u8g2.drawBox(progressBarX + 1, progressBarY + 1, fillWidth - 2, progressBarHeight - 2);
    }

    // Draw percentage text
    char percentText[8];
    snprintf(percentText, sizeof(percentText), "%d%%", currentProgress);
    u8g2.setFont(u8g2_font_6x10_tr);
    int percentWidth = u8g2.getStrWidth(percentText);
    int percentX = (128 - percentWidth) / 2;
    u8g2.drawStr(percentX, progressBarY - 3, percentText);

    u8g2.sendBuffer();

    delay(frameDelay);
  }

  // Ensure we end at exactly the target progress
  showLoadingProgress(title, toProgress, statusText);
}
