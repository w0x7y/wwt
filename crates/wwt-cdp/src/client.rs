//! A minimal CDP client: request/response correlation over one websocket.
//!
//! M1 discards protocol events. The event pump that feeds the extraction loop
//! is M2; it hooks into `read_loop` below without changing this API.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::{Duration, timeout};

/// Every command carries a deadline, so a wedged page cannot hang the caller.
/// Spec section 8.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A CDP protocol event: any message the browser sends that is not a
/// response to one of our commands.
#[derive(Debug, Clone)]
pub struct Event {
    /// `None` for browser-level events, `Some` for events from an attached
    /// page session.
    pub session_id: Option<String>,
    pub method: String,
    pub params: Value,
}

/// A plain `std` mutex, not tokio's: nothing awaits while it is held, and
/// `subscribe` is far more useful synchronous than async.
type Subscribers = Arc<StdMutex<Vec<mpsc::UnboundedSender<Event>>>>;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub struct Client {
    next_id: AtomicU64,
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    subscribers: Subscribers,
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
        let subscribers: Subscribers = Arc::new(StdMutex::new(Vec::new()));
        tokio::spawn(read_loop(
            stream,
            Arc::clone(&pending),
            Arc::clone(&subscribers),
        ));

        Ok(Self {
            next_id: AtomicU64::new(1),
            outgoing: tx,
            pending,
            subscribers,
        })
    }

    /// Receive every protocol event from now on.
    ///
    /// Subscribe *before* issuing the command whose event you intend to
    /// wait for, or you can miss it.
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribers
            .lock()
            .expect("the subscriber list is never held across a panic")
            .push(tx);
        rx
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

        let mut response = match timeout(CALL_TIMEOUT, rx).await {
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

        // Taken rather than cloned: an extraction's result is every run on
        // screen, and this is the first of two places that used to deep-copy
        // the whole of it on the way to the caller.
        Ok(response
            .get_mut("result")
            .map(Value::take)
            .unwrap_or_else(|| json!({})))
    }
}

async fn read_loop<S>(mut stream: S, pending: Pending, subscribers: Subscribers)
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

        // Messages with an `id` are responses to our commands; everything
        // else is an event.
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(value);
            }
            continue;
        }

        let Some(method) = value.get("method").and_then(Value::as_str) else {
            continue;
        };
        let event = Event {
            session_id: value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string),
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or_else(|| json!({})),
        };

        // A subscriber whose receiver is gone is pruned rather than left to
        // accumulate for the life of the connection.
        subscribers
            .lock()
            .expect("the subscriber list is never held across a panic")
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    // The socket is gone; wake every caller rather than letting them wait out
    // their deadlines.
    pending.lock().await.clear();
    subscribers
        .lock()
        .expect("the subscriber list is never held across a panic")
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use tokio_tungstenite::tungstenite::{Error as WsError, Message};

    fn parts() -> (Pending, Subscribers) {
        (
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(StdMutex::new(Vec::new())),
        )
    }

    fn one(text: &str) -> impl futures_util::Stream<Item = Result<Message, WsError>> + Unpin {
        stream::iter(vec![Ok(Message::text(text.to_string()))])
    }

    #[tokio::test]
    async fn events_reach_subscribers() {
        let (pending, subs) = parts();
        let (tx, mut rx) = mpsc::unbounded_channel();
        subs.lock().unwrap().push(tx);

        read_loop(
            one(r#"{"method":"Page.loadEventFired","sessionId":"S1","params":{"timestamp":1}}"#),
            pending,
            subs,
        )
        .await;

        let event = rx.recv().await.expect("an event");
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session_id.as_deref(), Some("S1"));
        assert_eq!(event.params["timestamp"], 1);
    }

    #[tokio::test]
    async fn an_event_without_a_session_is_still_delivered() {
        let (pending, subs) = parts();
        let (tx, mut rx) = mpsc::unbounded_channel();
        subs.lock().unwrap().push(tx);

        read_loop(one(r#"{"method":"Target.targetCreated","params":{}}"#), pending, subs).await;

        let event = rx.recv().await.expect("an event");
        assert_eq!(event.method, "Target.targetCreated");
        assert!(event.session_id.is_none());
    }

    #[tokio::test]
    async fn responses_still_correlate_while_events_flow() {
        let (pending, subs) = parts();
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);

        let messages = vec![
            Ok::<_, WsError>(Message::text(r#"{"method":"Page.loadEventFired","params":{}}"#.to_string())),
            Ok(Message::text(r#"{"id":7,"result":{"ok":true}}"#.to_string())),
        ];
        read_loop(stream::iter(messages), pending, subs).await;

        let response = rx.await.expect("the response");
        assert_eq!(response["result"]["ok"], true);
    }

    #[tokio::test]
    async fn a_dead_subscriber_does_not_stop_delivery_to_a_live_one() {
        // The pruning itself is memory hygiene and not observable from here —
        // what must hold is that dropping one subscriber never costs another
        // its events.
        let (pending, subs) = parts();
        let (dead_tx, dead_rx) = mpsc::unbounded_channel::<Event>();
        let (live_tx, mut live_rx) = mpsc::unbounded_channel::<Event>();
        subs.lock().unwrap().push(dead_tx);
        subs.lock().unwrap().push(live_tx);
        drop(dead_rx);

        let messages = vec![
            Ok::<_, WsError>(Message::text(r#"{"method":"Page.loadEventFired","params":{}}"#.to_string())),
            Ok(Message::text(r#"{"method":"Page.frameNavigated","params":{}}"#.to_string())),
        ];
        read_loop(stream::iter(messages), pending, subs).await;

        assert_eq!(live_rx.recv().await.expect("first").method, "Page.loadEventFired");
        assert_eq!(live_rx.recv().await.expect("second").method, "Page.frameNavigated");
    }

    #[tokio::test]
    async fn a_closed_socket_closes_every_subscriber() {
        // Otherwise a subscriber waits on a connection that will never speak
        // again, instead of learning that the browser is gone.
        let (pending, subs) = parts();
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        subs.lock().unwrap().push(tx);

        read_loop(one(r#"{"method":"Page.loadEventFired","params":{}}"#), pending, subs).await;

        assert!(rx.recv().await.is_some(), "the event itself");
        assert!(rx.recv().await.is_none(), "the channel should close with the socket");
    }
}
