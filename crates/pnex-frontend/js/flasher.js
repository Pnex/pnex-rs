// esptool-js glue for flashing firmware from the browser (Web Serial —
// Chromium only). This module is the ONLY JS interop point of the front:
// bundled by esbuild as an IIFE (`bun run js:build` → assets/flasher.js) and
// loaded as a classic script by App (main.rs). It exposes two globals
// consumed by src/flash.rs (wasm-bindgen):
//
//   window.pnexFlashSupported() -> boolean
//   window.pnexFlash(entries: [{data: Uint8Array, address: number}],
//                    onEvent: (json: string) => void) -> Promise
//
// onEvent receives JSON strings {type:"stage"|"chip"|"progress"|"done"|"error", ...}
// parsed on the Rust side with serde_json (no serde-wasm-bindgen dependency).
//
// The image served by /api/v1/download/firmware/{id} is ALWAYS a merged
// image flashable @0x0 (see pnex-firmware-builder/src/merge.rs: esp8266
// single image @0x0; esp32 bootloader+partitions+app merged) → a single
// writeFlash at address 0. Parameters aligned with the server merge
// (--flash-mode dio --flash-freq 40m --flash-size 4MB).

import { ESPLoader, Transport } from "esptool-js";

const emit = (onEvent, event) => onEvent(JSON.stringify(event));

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Port of esptool.py's UnixTightReset: every transition writes DTR and RTS in
// ONE setSignals call. esptool-js 0.6.1 only ships ClassicReset, which flips
// the lines one at a time and walks through invalid intermediate pairs; the
// ESP32-CAM-MB carrier (CH340 + two-transistor auto-reset) misses the
// download-mode entry that way ("Failed to connect with the device"), while
// esptool.py — tight reset first on Linux/macOS — syncs on the first try.
// true = asserted = line LOW (EN/IO0 pulled down through the transistors).
class TightReset {
  constructor(transport, resetDelay) {
    this.transport = transport;
    this.resetDelay = resetDelay;
  }

  async set(dtr, rts) {
    // Keep Transport.setRTS's DTR workaround consistent with the real state.
    this.transport._DTR_state = dtr;
    await this.transport.device.setSignals({
      dataTerminalReady: dtr,
      requestToSend: rts,
    });
  }

  async reset() {
    await this.set(false, false);
    await this.set(true, true);
    await this.set(false, true); // IO0=HIGH, EN=LOW: chip held in reset
    await sleep(100);
    await this.set(true, false); // IO0=LOW, EN=HIGH: boots into download mode
    await sleep(this.resetDelay);
    await this.set(false, false); // release IO0
  }
}

// Same strategy order as esptool.py on POSIX: tight resets first (short then
// long delay), classic resets as fallback for adapters that dislike atomic
// writes. The non-USB-JTAG branch only; native USB chips keep esptool-js's
// own sequence.
class PnexESPLoader extends ESPLoader {
  constructResetSequence(mode) {
    const base = super.constructResetSequence(mode);
    if (mode === "no_reset" || this.transport.getPid() === this.USB_JTAG_SERIAL_PID) {
      return base;
    }
    return [new TightReset(this.transport, 50), new TightReset(this.transport, 550), ...base];
  }
}

const ROM_BAUD = 115200;
const FAST_BAUD = 921600;

// esptool-js main() wraps the first command after the baud switch in this
// message; any failure there means the switch itself lost the chip.
const isBaudSwitchFailure = (err) =>
  String(err && err.message ? err.message : err).includes("Unable to verify flash chip connection");

window.pnexFlashSupported = () => "serial" in navigator;

window.pnexFlash = async (entries, onEvent) => {
  // entries = [{ data: Uint8Array, address: number }, ...] — a single
  // multi-entry writeFlash: merged firmware @0x0 (+ PNEXCFG1 sector
  // @0x200000 for the generic firmware, Brick 0 B0.1).
  // requestPort() requires a user gesture: this call must start from the
  // click handler, with no network wait in between (the firmware bytes
  // are downloaded when the modal opens, not on click).
  const port = await navigator.serial.requestPort();
  let transport = new Transport(port, true);

  try {
    emit(onEvent, { type: "stage", stage: "connect" });
    let loader = new PnexESPLoader({ transport, baudrate: FAST_BAUD });
    let chip;
    try {
      // main() opens the port, detects the chip, loads the stub and raises
      // the baud rate.
      chip = await loader.main();
    } catch (err) {
      if (!isBaudSwitchFailure(err)) throw err;
      // Web Serial can only change the baud rate by closing and reopening
      // the port; on some carriers (ESP32-CAM-MB) the DTR/RTS toggles of
      // that reopen reset the chip and the RAM stub is gone ("Serial data
      // stream stopped"). Reconnect on the same port and stay at the ROM
      // baud rate: no reopen, slower but reliable.
      await transport.disconnect().catch(() => {});
      transport = new Transport(port, true);
      loader = new PnexESPLoader({ transport, baudrate: ROM_BAUD });
      chip = await loader.main();
    }
    emit(onEvent, { type: "chip", chip });

    emit(onEvent, { type: "stage", stage: "write" });
    await loader.writeFlash({
      fileArray: entries.map((e) => ({ data: e.data, address: e.address })),
      flashMode: "dio",
      flashFreq: "40m",
      flashSize: "4MB",
      eraseAll: false,
      compress: true,
      reportProgress: (_fileIndex, written, total) =>
        emit(onEvent, {
          type: "progress",
          percent: total > 0 ? Math.round((written / total) * 100) : 0,
        }),
    });

    // Hardware reset so the board boots the new firmware. esptool-js 0.6.1
    // HardReset only RELEASES RTS (setRTS(false)) without asserting it
    // first, so EN never goes low and the board kept running the stub until
    // a manual press. Explicit esptool.py sequence instead: DTR=0 keeps
    // GPIO0 high (normal boot, not download mode), RTS=1 pulls EN low,
    // hold 100 ms, RTS=0 releases EN.
    emit(onEvent, { type: "stage", stage: "reset" });
    await transport.setDTR(false);
    await transport.setRTS(true);
    await sleep(100);
    await transport.setRTS(false);

    emit(onEvent, { type: "done" });
  } catch (err) {
    // Port picker cancelled (NotFoundError), port busy, sync failed…:
    // reported as an "error" event (readable message on the Rust side)
    // AND a promise rejection — Rust reads either one.
    const message = err && err.message ? String(err.message) : String(err);
    emit(onEvent, { type: "error", message });
    throw err;
  } finally {
    await transport.disconnect().catch(() => {});
  }
};
