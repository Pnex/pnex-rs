//
// pnex-ws-tcp — TCP clients handed to ArduinoWebsockets (O20).
//
// The library reads the HTTP 101 handshake response with a hard-coded
// 1000 ms budget (`_CONNECTION_TIMEOUT`, not overridable by -D): on a WiFi
// link with a 100–500 ms RTT the device gives up before the answer, nginx
// logs 499 and the device reconnects in a loop. Instead of patching the
// (GPL-3.0) library, PneX gives every WebsocketsClient its own TCP client
// whose readLine() waits PNEX_WS_HANDSHAKE_TIMEOUT_MS and yields while
// waiting (an ESP8266 busy loop over ~3 s trips the soft watchdog).
//

#ifndef PNEX_WS_TCP_H
#define PNEX_WS_TCP_H

#include <Arduino.h>
#include <ArduinoWebsockets.h>

#include <memory>

#ifndef PNEX_WS_HANDSHAKE_TIMEOUT_MS
#define PNEX_WS_HANDSHAKE_TIMEOUT_MS 5000
#endif

// Wraps a library TCP client (plain or TLS): same behaviour, patient
// handshake line reads.
template <class Base>
class PnexPatientTcp : public Base {
public:
    websockets::WSString readLine() override {
        websockets::WSString line;
        const unsigned long start = millis();
        while (this->available()) {  // = still connected
            if (millis() - start > PNEX_WS_HANDSHAKE_TIMEOUT_MS) {
                return "";
            }
            const int ch = this->client.read();
            if (ch < 0) {
                delay(1);  // nothing yet: let WiFi/TCP run (and the WDT)
                continue;
            }
            line += (char)ch;
            if (ch == '\n') {
                break;
            }
        }
        return line;
    }
};

#if defined(ESP32)
using PnexWsPlainTcp = PnexPatientTcp<websockets::network::Esp32TcpClient>;
using PnexWsTlsTcp = PnexPatientTcp<websockets::network::SecuredEsp32TcpClient>;
#else
using PnexWsPlainTcp = PnexPatientTcp<websockets::network::Esp8266TcpClient>;
using PnexWsTlsTcp = PnexPatientTcp<websockets::network::SecuredEsp8266TcpClient>;
#endif

// Opens `url` ("ws[s]://host[:port]/path?query") on a fresh WebsocketsClient
// built over a patient TCP client, with the shared TLS posture (pnex_tls)
// for wss. `client` is replaced: the callbacks are bound again here.
bool pnex_ws_open(websockets::WebsocketsClient& client, const char* url,
                  websockets::PartialMessageCallback on_message,
                  websockets::PartialEventCallback on_event);

#endif  // PNEX_WS_TCP_H
