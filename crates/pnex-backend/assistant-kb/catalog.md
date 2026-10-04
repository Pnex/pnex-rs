---
id: catalog
title: Device Catalog
kind: feature
pages: /catalog
nodes: 
err_codes: 
tools: 
tags: catalog, models, boards, pinout, esp32, esp8266, capabilities, catalogue, modèle, carte
---
The Device Catalog page (Edges menu) lists every device model the platform supports, with its board, type and capabilities, so you can choose hardware before registering a device. It is a read-only reference; registration itself happens on the Devices / Agents page.

## What you can do

- Search by name, description, board or capability, and filter by device type (Sensor, Actuator, Mixed) or by board.
- Read each model's revision, board and capabilities in the table.
- Click **View pinout** to open the interactive drawing of the board: pin names, roles (GPIO, ADC, touch, DAC, I²C, SPI, UART, power, GND, EN/RST) and pins reserved by a built-in screen.

## Model families

When you register a device (Devices / Agents → **Register**, step Model) the catalog models come in three groups:

- **Generic PneX**: inputs and outputs are driven from the UI with no code, or you attach your own firmware written on the Custom firmware page.
- **Predefined boards**: ready-to-use boards whose firmware is maintained by PneX and fixed by the model (no firmware choice).
- **Agents**: software on a computer or Raspberry Pi that pushes values; computers are not boards.

## Good to know

- The board of a device is frozen at registration: when a model has several physical variants, pick the right **Board variant** in the wizard.
- The pinout follows a fixed convention: the USB connector is drawn at the top.
- Every firmware is compiled per device by the server; there is no reusable generic binary to download from the catalog.
