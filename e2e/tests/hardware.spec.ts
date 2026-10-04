// Real boards, one serial sequence (a board is never driven by two tests
// at once, and registration comes first). Skipped unless the boards and
// Wi-Fi are declared (README › Hardware). The test bodies live in hw/.
import { test } from '../src/fixtures.ts';
import { cameraTests } from './hw/camera.ts';
import { customFirmwareTests } from './hw/custom-fw.ts';
import { otaTests } from './hw/ota.ts';
import { pinIoTest, pinsTests } from './hw/pins.ts';
import { registerAndFlash } from './hw/register.ts';

test.describe('hardware', { tag: '@hardware' }, () => {
  test.describe.configure({ mode: 'default' });
  registerAndFlash('c3');
  pinsTests();
  otaTests('c3');
  customFirmwareTests('c3');
  registerAndFlash('cam');
  cameraTests();
  // ESP32 DevKit 38 pins + TFT: G25 output, G34 (ADC1, input-only) analog.
  registerAndFlash('esp32');
  pinIoTest('esp32', 25, 34);
  // ESP32 DevKit 38 pins WROOM-32U (external antenna) + TFT: same pin map.
  registerAndFlash('esp32u');
  pinIoTest('esp32u', 25, 34);
  // NodeMCU V3 + soldered OLED (ESP8266): D7 = GPIO13 output, A0 (wire id 17).
  registerAndFlash('nodemcu');
  pinIoTest('nodemcu', 13, 17);
  // Waveshare ESP32-C6-Zero (Arduino core 3.x): GP14 output, GP0 (ADC1).
  registerAndFlash('c6');
  pinIoTest('c6', 14, 0);
  otaTests('c6');
  customFirmwareTests('c6');
});
