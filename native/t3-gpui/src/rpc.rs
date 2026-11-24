//! Effect RPC client over the `/ws` socket, JSON serialization.
//! Wire shapes: `.repos/effect/packages/effect/src/rpc/RpcMessage.ts`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Marks every part of a split frame but the last (`packages/shared/src/socketFrameParts.ts`).
const PART_PREFIX: char = '\u{1e}';
const PING_EVERY: Duration = Duration::from_secs(5);

type Pending = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Result<Value>>>>>;

#[derive(Clone)]
pub struct RpcClient {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    shutdown: Arc<tokio::sync::Notify>,
}

impl RpcClient {
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (socket, _) = tokio_tungstenite::connect_async(ws_url).await?;
        let (mut sink, mut source) = socket.split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::default();
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let writer_shutdown = shutdown.clone();

        tokio::spawn(async move {
            let mut ping = tokio::time::interval(PING_EVERY);
            loop {
                let frame = tokio::select! {
                    Some(frame) = out_rx.recv() => frame,
                    _ = ping.tick() => json!({"_tag": "Ping"}).to_string(),
                    _ = writer_shutdown.notified() => {
                        let _ = sink.close().await;
                        break;
                    }
                    else => break,
                };
                if sink.send(Message::Text(frame)).await.is_err() {
                    break;
                }
            }
        });

        let reader_pending = pending.clone();
        let reader_out = out.clone();
        tokio::spawn(async move {
            let mut partial = String::new();
            while let Some(Ok(msg)) = source.next().await {
                let Message::Text(text) = msg else { continue };
                if let Some(part) = text.strip_prefix(PART_PREFIX) {
                    partial.push_str(part);
                    continue;
                }
                let whole = if partial.is_empty() { text } else { std::mem::take(&mut partial) + &text };
                match serde_json::from_str::<Value>(&whole) {
                    Ok(value) => dispatch(value, &reader_pending, &reader_out),
                    Err(err) => eprintln!("rpc: undecodable frame: {err}"),
                }
            }
            for (_, tx) in reader_pending.lock().unwrap().drain() {
                let _ = tx.send(Err(anyhow!("connection closed")));
            }
        });

        Ok(Self { out, pending, next_id: Arc::default(), shutdown })
    }

    /// Starts a request; the receiver yields stream chunks, then ends after `Exit`.
    /// A unary call yields exactly one value (the success value).
    pub fn request(&self, tag: &str, payload: Value) -> (String, mpsc::UnboundedReceiver<Result<Value>>) {
        let id = (self.next_id.fetch_add(1, Ordering::Relaxed) + 1).to_string();
        let (tx, rx) = mpsc::unbounded_channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let frame = json!({"_tag": "Request", "id": id, "tag": tag, "payload": payload, "headers": []});
        let _ = self.out.send(frame.to_string());
        (id, rx)
    }

    /// Stops a stream on the server; its receiver ends.
    pub fn cancel(&self, id: &str) {
        if self.pending.lock().unwrap().remove(id).is_some() {
            let _ = self.out.send(json!({"_tag": "Interrupt", "requestId": id}).to_string());
        }
    }

    pub fn close(&self) {
        self.shutdown.notify_one();
    }

    pub async fn call(&self, tag: &str, payload: Value) -> Result<Value> {
        self.request(tag, payload).1.recv().await.unwrap_or_else(|| Err(anyhow!("{tag}: no response")))
    }
}

fn dispatch(msg: Value, pending: &Pending, out: &mpsc::UnboundedSender<String>) {
    let id = match &msg["requestId"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    match msg["_tag"].as_str() {
        Some("Chunk") => {
            if let Some(tx) = pending.lock().unwrap().get(&id) {
                for value in msg["values"].as_array().into_iter().flatten() {
                    let _ = tx.send(Ok(value.clone()));
                }
            }
            // Streams are backpressured: the server waits for an Ack per chunk batch.
            let _ = out.send(json!({"_tag": "Ack", "requestId": id}).to_string());
        }
        Some("Exit") => {
            let Some(tx) = pending.lock().unwrap().remove(&id) else { return };
            let exit = &msg["exit"];
            match exit["_tag"].as_str() {
                Some("Success") if !exit["value"].is_null() => {
                    let _ = tx.send(Ok(exit["value"].clone()));
                }
                Some("Success") => {}
                _ => {
                    let _ = tx.send(Err(anyhow!("{}", describe_failure(exit))));
                }
            }
        }
        Some("Defect") | Some("ClientProtocolError") => {
            let err = msg.to_string();
            for (_, tx) in pending.lock().unwrap().drain() {
                let _ = tx.send(Err(anyhow!("{err}")));
            }
        }
        _ => {}
    }
}

fn describe_failure(exit: &Value) -> String {
    let causes = exit["cause"].as_array().cloned().unwrap_or_default();
    let first = causes.first().cloned().unwrap_or(Value::Null);
    let detail = match first["_tag"].as_str() {
        Some("Fail") => &first["error"],
        Some("Die") => &first["defect"],
        _ => &first,
    };
    detail["message"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| detail["_tag"].as_str().map(str::to_owned))
        .unwrap_or_else(|| detail.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_is_delivered_and_acked_then_exit_ends_stream() {
        let pending: Pending = Arc::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        pending.lock().unwrap().insert("7".into(), tx);
        let (out, mut out_rx) = mpsc::unbounded_channel();

        dispatch(json!({"_tag": "Chunk", "requestId": "7", "values": [1, 2]}), &pending, &out);
        assert_eq!(rx.try_recv().unwrap().unwrap(), json!(1));
        assert_eq!(rx.try_recv().unwrap().unwrap(), json!(2));
        assert_eq!(out_rx.try_recv().unwrap(), r#"{"_tag":"Ack","requestId":"7"}"#);

        let fail = json!({"_tag": "Exit", "requestId": "7", "exit": {"_tag": "Failure", "cause": [{"_tag": "Fail", "error": {"_tag": "E", "message": "nope"}}]}});
        dispatch(fail, &pending, &out);
        assert_eq!(rx.try_recv().unwrap().unwrap_err().to_string(), "nope");
        assert!(pending.lock().unwrap().is_empty());
    }
}
