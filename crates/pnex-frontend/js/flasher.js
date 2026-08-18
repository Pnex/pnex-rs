// Glue esptool-js pour le flash firmware depuis le navigateur (Web Serial —
// Chromium uniquement). Ce module est le SEUL point d'interop JS du front :
// bundlé par esbuild en IIFE (`npm run js:build` → assets/flasher.js) et
// chargé comme script classique par App (main.rs). Il expose deux globales
// consommées par src/flash.rs (wasm-bindgen) :
//
//   window.pnexFlashSupported() -> boolean
//   window.pnexFlash(bytes: Uint8Array, onEvent: (json: string) => void) -> Promise
//
// onEvent reçoit des chaînes JSON {type:"stage"|"chip"|"progress"|"done"|"error", ...}
// parsées côté Rust avec serde_json (pas de dépendance serde-wasm-bindgen).
//
// L'image servie par /api/v1/download/firmware/{id} est TOUJOURS une image
// mergée flashable @0x0 (cf. pnex-firmware-builder/src/merge.rs : esp8266
// image unique @0x0 ; esp32 bootloader+partitions+app mergées) → un seul
// writeFlash à l'adresse 0. Paramètres alignés sur le merge serveur
// (--flash-mode dio --flash-freq 40m --flash-size 4MB).

import { ESPLoader, Transport, HardReset } from "esptool-js";

const emit = (onEvent, event) => onEvent(JSON.stringify(event));

window.pnexFlashSupported = () => "serial" in navigator;

window.pnexFlash = async (bytes, onEvent) => {
  // requestPort() exige un geste utilisateur : cet appel doit partir du
  // handler du clic, sans attente réseau intermédiaire (les octets firmware
  // sont téléchargés à l'ouverture du modal, pas au clic).
  const port = await navigator.serial.requestPort();
  const transport = new Transport(port, true);

  try {
    emit(onEvent, { type: "stage", stage: "connect" });
    const loader = new ESPLoader({ transport, baudrate: 921600 });
    // main() ouvre le port, détecte le chip, charge le stub et monte le baud.
    const chip = await loader.main();
    emit(onEvent, { type: "chip", chip });

    emit(onEvent, { type: "stage", stage: "write" });
    await loader.writeFlash({
      fileArray: [{ data: bytes, address: 0x0 }],
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

    // Redémarrage matériel : la carte démarre sur le nouveau firmware.
    emit(onEvent, { type: "stage", stage: "reset" });
    await new HardReset(transport, true).reset();

    emit(onEvent, { type: "done" });
  } catch (err) {
    // Annulation du sélecteur de port (NotFoundError), port occupé, sync
    // échouée… : remontées comme événement "error" (message lisible côté
    // Rust) ET rejet de la promesse — Rust lit l'un ou l'autre.
    const message = err && err.message ? String(err.message) : String(err);
    emit(onEvent, { type: "error", message });
    throw err;
  } finally {
    await transport.disconnect().catch(() => {});
  }
};
