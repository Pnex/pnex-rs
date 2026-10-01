# tft_st7735_demo

Démo autonome : **ESP32-WROOM** (devkit) + écran **TFT 1.77"** (ST7735, SPI, 160×128).
Affiche en continu : valeur ADC brute (GPIO34), tension, heap libre, uptime, et une barre
graph de l'ADC.

## Câblage (VSPI, devkit ESP32 standard)

| TFT    | ESP32      |
|--------|------------|
| VCC    | 3V3        |
| GND    | GND        |
| SCK    | GPIO 18    |
| SDA (MOSI) | GPIO 23 |
| CS     | GPIO 5     |
| DC (RS, A0)| GPIO 2 |
| RST (RES)  | GPIO 4 |
| LEDA (BLK) | 3V3     |

`GPIO34` est l'entrée analogique lue par la démo : pont diviseur ou potentiomètre entre
3V3 et GND, curseur sur GPIO34. Non branchée, la valeur flotte (c'est normal).

## Build & flash

```bash
pio run -d firmware/tft_st7735_demo --target upload --upload-port /dev/ttyUSB0
pio device monitor -b 115200
```

ou via task : `task flash` / `task m` depuis ce dossier.

## Panneau capricieux ?

Les clones ST7735 varient. Si les couleurs sont inversées (rouge/bleu) ou si l'image est
décalée de quelques pixels, changer `TFT_TAB` dans `src/main.cpp` :

- `INITR_BLACKTAB` (défaut, la plupart des 1.8"/1.77")
- `INITR_REDTAB`
- `INITR_GREENTAB`
- `INITR_GREENTAB160x80`

## Afficher ses propres valeurs

Utiliser `printValue(x, y, taille, texte, couleur)` — elle efface la zone puis écrit
(pas de rémanence entre deux rafraîchissements). Police de base : 6×8 px par caractère,
multipliée par `taille` (taille 2 → 12×16 px).
