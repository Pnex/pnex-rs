//! Minimal Chrome DevTools Protocol client (site-recipes.md D325): the
//! dozen commands the navigation rules use, over one WebSocket.
//!
//! Messages are read as `serde_json::Value`, never into generated types:
//! immune to the workspace's serde_json `arbitrary_precision` (starlark),
//! which broke a typed client's untagged enums. The Bearer token of an
//! endpoint goes on the upgrade request itself. Flat sessions
//! (`Target.attachToTarget {flatten}`) carry the tab commands.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// Longest wait for one command's answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest wait for the close frame to leave.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// One protocol event: session (none = browser), method, params.
#[derive(Debug, Clone)]
pub struct Event {
    pub session: Option<String>,
    pub method: String,
    pub params: Value,
}

/// A connection to a browser. Dropping it closes the socket with a
/// WebSocket close frame: Obscura counts a connection reset without one
/// against `--max-connections` for good (2026-10-30, every slot taken).
pub struct Cdp {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    events: broadcast::Sender<Event>,
    next: AtomicU64,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Cdp {
    fn drop(&mut self) {
        // The writer ends on its own once `out` is dropped, after the
        // close frame.
        self.reader.abort();
    }
}

impl Cdp {
    /// Opens the browser socket (`ws(s)://…/devtools/browser/…`).
    pub async fn connect(ws_url: &str, token: Option<&str>) -> Result<Arc<Self>, String> {
        let mut request = ws_url.into_client_request().map_err(|e| e.to_string())?;
        if let Some(t) = token {
            let v = format!("Bearer {t}")
                .parse()
                .map_err(|_| "token is not a valid header value".to_string())?;
            request.headers_mut().insert("authorization", v);
        }
        let (ws, _) = tokio::time::timeout(CALL_TIMEOUT, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| "browser endpoint timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let (mut sink, mut stream) = ws.split();
        let (out, mut rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(1024);
        tokio::spawn(async move {
            while let Some(text) = rx.recv().await {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
                let _ = sink.send(Message::Close(None)).await;
                let _ = sink.close().await;
            })
            .await;
        });
        let reader = {
            let pending = pending.clone();
            let events = events.clone();
            tokio::spawn(async move {
                while let Some(Ok(msg)) = stream.next().await {
                    let Message::Text(text) = msg else {
                        continue;
                    };
                    let Ok(v) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if let Some(id) = v.get("id").and_then(Value::as_u64) {
                        let answer = match v.get("error") {
                            Some(e) => Err(e
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("CDP error")
                                .to_string()),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        if let Some(tx) = pending.lock().expect("pending").remove(&id) {
                            let _ = tx.send(answer);
                        }
                    } else if let Some(method) = v.get("method").and_then(Value::as_str) {
                        let _ = events.send(Event {
                            session: v
                                .get("sessionId")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            method: method.to_string(),
                            params: v.get("params").cloned().unwrap_or(Value::Null),
                        });
                    }
                }
                // Socket closed: every waiting call fails now.
                pending.lock().expect("pending").clear();
            })
        };
        Ok(Arc::new(Self {
            out,
            pending,
            events,
            next: AtomicU64::new(1),
            reader,
        }))
    }

    /// Sends a command (to a tab session when given) and waits its answer.
    pub async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.call_within(session, method, params, CALL_TIMEOUT)
            .await
    }

    pub async fn call_within(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending").insert(id, tx);
        if self.out.send(msg.to_string()).is_err() {
            self.pending.lock().expect("pending").remove(&id);
            return Err("browser connection closed".into());
        }
        match tokio::time::timeout(limit, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err("browser connection closed".into()),
            Err(_) => {
                self.pending.lock().expect("pending").remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}

/// A tab in its own browser context (no cookie kept between runs).
pub struct Tab {
    pub cdp: Arc<Cdp>,
    pub session: String,
    /// Target id; also the id of the tab's main frame.
    pub target: String,
    context: String,
}

impl Tab {
    pub async fn open(cdp: Arc<Cdp>) -> Result<Self, String> {
        let context = cdp
            .call(
                None,
                "Target.createBrowserContext",
                json!({ "disposeOnDetach": true }),
            )
            .await?["browserContextId"]
            .as_str()
            .ok_or("no browserContextId")?
            .to_string();
        let target = cdp
            .call(
                None,
                "Target.createTarget",
                json!({ "url": "about:blank", "browserContextId": context }),
            )
            .await?["targetId"]
            .as_str()
            .ok_or("no targetId")?
            .to_string();
        let session = cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target, "flatten": true }),
            )
            .await?["sessionId"]
            .as_str()
            .ok_or("no sessionId")?
            .to_string();
        Ok(Self {
            cdp,
            session,
            target,
            context,
        })
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.cdp.call(Some(&self.session), method, params).await
    }

    /// Value of a JavaScript expression (`returnByValue`), bounded: some
    /// engines (Obscura) do not answer while they load a page.
    pub async fn eval(&self, expression: &str, limit: Duration) -> Result<Value, String> {
        let r = self
            .cdp
            .call_within(
                Some(&self.session),
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
                limit,
            )
            .await?;
        if let Some(e) = r.get("exceptionDetails") {
            return Err(e
                .pointer("/exception/description")
                .or_else(|| e.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("script error")
                .to_string());
        }
        Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    /// Address of the tab, from the browser (answers while a page loads).
    pub async fn url(&self) -> String {
        self.cdp
            .call_within(
                None,
                "Target.getTargetInfo",
                json!({ "targetId": self.target }),
                Duration::from_secs(5),
            )
            .await
            .ok()
            .and_then(|r| {
                r.pointer("/targetInfo/url")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    /// JPEG of the viewport.
    pub async fn screenshot(&self) -> Result<Vec<u8>, String> {
        let r = self
            .call(
                "Page.captureScreenshot",
                json!({ "format": "jpeg", "quality": 70 }),
            )
            .await?;
        let data = r["data"].as_str().ok_or("no screenshot data")?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())
    }

    /// Closes the tab and its context.
    pub async fn close(&self) {
        let _ = self
            .cdp
            .call_within(
                None,
                "Target.disposeBrowserContext",
                json!({ "browserContextId": self.context }),
                Duration::from_secs(10),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dropped connection says goodbye: the browser gets a close frame,
    /// not a reset it may never count back.
    #[tokio::test]
    async fn dropping_the_connection_sends_a_close_frame() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            ws.next().await
        });
        let cdp = Cdp::connect(&format!("ws://{addr}/devtools/browser/x"), None)
            .await
            .unwrap();
        drop(cdp);
        let got = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(got, Some(Ok(Message::Close(_)))), "{got:?}");
    }
}
