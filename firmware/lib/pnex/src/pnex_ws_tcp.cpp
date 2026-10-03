//
// pnex-ws-tcp — implementation (see pnex_ws_tcp.h).
//
#include "pnex_ws_tcp.h"

#include "pnex_tls.h"

using namespace websockets;

bool pnex_ws_open(WebsocketsClient& client, const char* url,
                  PartialMessageCallback on_message,
                  PartialEventCallback on_event) {
    // "ws[s]://host[:port]/path?query" — same split as the library's own
    // connect(url): Host header without the port, default 80 / 443.
    const bool tls = strncmp(url, "wss://", 6) == 0;
    const char* rest = url + (tls ? 6 : (strncmp(url, "ws://", 5) == 0 ? 5 : 0));
    const char* slash = strchr(rest, '/');
    const size_t hostport_len = slash ? (size_t)(slash - rest) : strlen(rest);
    char host[96];
    snprintf(host, sizeof(host), "%.*s", (int)hostport_len, rest);
    int port = tls ? 443 : 80;
    char* colon = strrchr(host, ':');
    if (colon) {
        *colon = '\0';
        port = atoi(colon + 1);
    }
    const char* path = slash ? slash : "/";

    std::shared_ptr<network::TcpClient> tcp;
    if (tls) {
        auto secured = std::make_shared<PnexWsTlsTcp>();
#if defined(ESP32)
        // Same as the library's wss path: CA when pinned, core default
        // otherwise.
        if (const char* ca = pnex_tls_ca_pem()) {
            secured->setCACert(ca);
        }
#else
        // ArduinoWebsockets on ESP8266 takes no CA: insecure, as before.
        if (pnex_tls_ca_pem()) {
            Serial.println("[TLS] WS CA pinning unsupported on ESP8266 — insecure WS");
        }
        secured->setInsecure();
#endif
        tcp = secured;
    } else {
        tcp = std::make_shared<PnexWsPlainTcp>();
    }
    client = WebsocketsClient(tcp);
    client.onMessage(on_message);
    client.onEvent(on_event);
    return client.connect(host, port, path);
}
