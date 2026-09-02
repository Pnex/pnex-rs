// Secteur de config PNEXCFG1 (Brick 0, B0.1) — miroir exact du builder
// Rust `pnex_firmware_builder::config_sector` (brick0.md §5).
//
// Layout 4 Ko à l'offset 0x200000 :
//   0x00  magic  "PNEXCFG1"  (8 o)
//   0x08  version u16 LE (= 1)
//   0x0A  crc32  u32 LE — CRC IEEE du payload JSON
//   0x0E  payload JSON compact {wifi_ssid, wifi_password, host, token,
//         device_id, ws_ssl} — chaînes claires (pas de base64 : la
//         contrainte -D du build custom a disparu avec B0.1)
//   0x0E+len … 0x1000 rempli 0xFF (état flash direct effacée)
//
// Lecture par mots de 32 bits (ESP.flashRead exige l'alignement 4 o).

#ifndef GENERIC_CONFIG_SECTOR_H
#define GENERIC_CONFIG_SECTOR_H

#include <Arduino.h>
#include <ESP8266WiFi.h>

/// Offset flash du secteur (flash 4 Mo — hors zone SDK ; à re-valider au
/// premier flash réel, brick0.md §10).
static const uint32_t CONFIG_OFFSET = 0x00200000;
static const uint16_t CONFIG_VERSION = 1;
static const char CONFIG_MAGIC[8] = { 'P', 'N', 'E', 'X', 'C', 'F', 'G', '1' };

/// Config device décodée — bornes = limites de validation de l'API.
struct DeviceConfig {
    char wifi_ssid[65];
    char wifi_password[65];
    char host[65];
    char token[65];
    char device_id[65];
    bool ws_ssl;
};

/// Copie bornée d'une chaîne ArduinoJson → buffer fixe (toujours null-terminé).
static bool cfg_copy_str(char* out, size_t cap, const char* src) {
    if (!src) return false;
    size_t n = strlen(src);
    if (n >= cap) return false;
    memcpy(out, src, n + 1);
    return true;
}

/// CRC32 IEEE (0xEDB88320 poly réfléchi) — identique au builder Rust.
static uint32_t cfg_crc32(const uint8_t* data, size_t len) {
    uint32_t crc = 0xFFFFFFFFu;
    for (size_t i = 0; i < len; ++i) {
        crc ^= data[i];
        for (int b = 0; b < 8; ++b) {
            uint32_t mask = (uint32_t)-(int32_t)(crc & 1u);
            crc = (crc >> 1) ^ (0xEDB88320u & mask);
        }
    }
    return ~crc;
}

/// Lit `len` octets à `offset` (API bulk du core ESP8266).
static bool cfg_flash_read(uint32_t offset, uint8_t* out, size_t len) {
    return ESP.flashRead(offset, out, len);
}

/// Lit + parse le secteur → config. Faux si magie/version/CRC/JSON invalide.
static bool cfg_load(DeviceConfig& cfg) {
    static uint8_t sector[4096];
    static bool loaded = false;
    if (loaded) {
        return true;
    }
    if (!cfg_flash_read(CONFIG_OFFSET, sector, sizeof(sector))) {
        return false;
    }
    if (memcmp(sector, CONFIG_MAGIC, 8) != 0) {
        return false;
    }
    uint16_t version = (uint16_t)(sector[8] | (sector[9] << 8));
    if (version != CONFIG_VERSION) {
        return false;
    }
    uint32_t crc = (uint32_t)sector[10] | ((uint32_t)sector[11] << 8) |
                   ((uint32_t)sector[12] << 16) | ((uint32_t)sector[13] << 24);
    size_t end = 14;
    while (end < sizeof(sector) && sector[end] != 0xFF) {
        ++end;
    }
    if (end == 14 || cfg_crc32(sector + 14, end - 14) != crc) {
        return false;
    }
    char* json = (char*)(sector + 14);
    json[end - 14] = '\0';
    JsonDocument doc;
    if (deserializeJson(doc, json) != DeserializationError::Ok) {
        return false;
    }
    bool ok = true;
    ok &= cfg_copy_str(cfg.wifi_ssid, sizeof(cfg.wifi_ssid), doc["wifi_ssid"] | "");
    ok &= cfg_copy_str(cfg.wifi_password, sizeof(cfg.wifi_password), doc["wifi_password"] | "");
    ok &= cfg_copy_str(cfg.host, sizeof(cfg.host), doc["host"] | "");
    ok &= cfg_copy_str(cfg.token, sizeof(cfg.token), doc["token"] | "");
    ok &= cfg_copy_str(cfg.device_id, sizeof(cfg.device_id), doc["device_id"] | "");
    cfg.ws_ssl = doc["ws_ssl"] | false;
    if (ok) {
        loaded = true;
    }
    return ok;
}

#endif  // GENERIC_CONFIG_SECTOR_H
