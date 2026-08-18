//! A minimal CDP client: request/response correlation over one websocket.
//!
//! M1 discards protocol events. The event pump that feeds the extraction loop
//! is M2; it hooks into `read_loop` below without changing this API.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::{Duration, timeout};

/// Every command carries a deadline, so a wedged page cannot hang the caller.
/// Spec section 8.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub struct Client {
    next_id: AtomicU64,
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
}

impl Client {
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .with_context(|| format!("failed to connect to {ws_url}"))?;
        let (mut sink, stream) = stream.split();

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sink.send(msg.into()).await.is_err() {
                    break;
                }
            }
        });

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(read_loop(stream, Arc::clone(&pending)));

        Ok(Self {
            next_id: AtomicU64::new(1),
            outgoing: tx,
            pending,
        })
    }

    /// Send a command to the browser target.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.send(method, params, None).await
    }

    /// Send a command to an attached session (a page).
    pub async fn call_on(&self, session_id: &str, method: &str, params: Value) -> Result<Value> {
        self.send(method, params, Some(session_id)).await
    }

    async fn send(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(session_id) = session {
            msg["sessionId"] = json!(session_id);
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        self.outgoing
            .send(msg.to_string())
            .map_err(|_| anyhow!("the CDP connection is closed"))?;

        let response = match timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => {
                return Err(anyhow!("the CDP connection closed while awaiting {method}"));
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(anyhow!("{method} timed out after {CALL_TIMEOUT:?}"));
            }
        };

        if let Some(error) = response.get("error") {
            let message = error["message"].as_str().unwrap_or("unknown error");
            let data = error["data"].as_str().unwrap_or_default();
            return Err(anyhow!("{method} failed: {message} {data}").context(method.to_string()));
        }

        Ok(response
            .get("result")
            .cloned()
            .unwrap_or_else(|| json!({})))
    }
}

async fn read_loop<S>(mut stream: S, pending: Pending)
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    while let Some(Ok(msg)) = stream.next().await {
        let Ok(text) = msg.into_text() else { continue };
        let Ok(value): Result<Value, _> = serde_json::from_str(&text) else {
            continue;
        };
        // Messages with an `id` are responses; everything else is an event,
        // which M1 drops on the floor.
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if let Some(tx) = pending.lock().await.remove(&id) {
            let _ = tx.send(value);
        }
    }
    // The socket is gone; wake every caller rather than letting them wait out
    // their deadlines.
    pending.lock().await.clear();
}
