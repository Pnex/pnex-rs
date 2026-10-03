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
  otaTests();
  customFirmwareTests();
  registerAndFlash('cam');
  cameraTests();
  // ESP32 DevKit 38 pins + TFT: G25 output, G34 (ADC1, input-only) analog.
  registerAndFlash('esp32');
  pinIoTest('esp32', 25, 34);
});
