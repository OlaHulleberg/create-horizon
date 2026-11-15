//! Create's network messages: one play payload, `create:net`, both ways. Each message carries
//! the name of the feature's channel (`create:depot`, `create:kinetics`...) and its bytes, so
//! the engine's encoders keep their channels and the glue declares one payload.
//!
//! A send the connection cannot take now (backpressure) waits in an outbox, per player in the
//! order sent, and goes first at the next level tick.

use super::subsystem::Subsystem;
use mod_api::{
    ClientHost, IPayloadContext, IPayloadHandler, PayloadBytes, PayloadHandleError, PayloadId,
    PayloadReader, PayloadSendError, PayloadWriter, PlayerRef, Registrar, ServerHost,
};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// The one payload's id.
pub const PAYLOAD: &str = "create:net";

/// Its schema version: bump when a message's layout changes.
const VERSION: &str = "1";

/// The largest message body.
const MAX_BODY: usize = 1 << 20;

fn id() -> PayloadId {
    PayloadId(PAYLOAD.to_owned())
}

fn encode(channel: &str, data: &[u8]) -> PayloadBytes {
    let mut w = PayloadWriter::new(MAX_BODY + 4096);
    w.string(channel);
    w.bytes(data, MAX_BODY);
    // Within the limit by construction: a channel name and at most `MAX_BODY` bytes.
    PayloadBytes(w.finish().unwrap_or_default())
}

fn decode(bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    let mut r = PayloadReader::new(bytes);
    let channel = r.string().ok()?;
    let data = r.bytes(MAX_BODY).ok()?.to_vec();
    r.finish().ok()?;
    Some((channel, data))
}

/// The server's receiver: hands each message to the subsystem that owns its channel.
struct FromClient(&'static [&'static dyn Subsystem]);

impl IPayloadHandler for FromClient {
    fn handle(
        &self,
        payload: PayloadBytes,
        cx: &mut dyn IPayloadContext,
    ) -> Result<(), PayloadHandleError> {
        let Some((channel, data)) = decode(&payload.0) else {
            return Err(PayloadHandleError::Failed(
                "malformed create:net message".into(),
            ));
        };
        let player = cx
            .player()
            .map_err(|e| PayloadHandleError::Failed(format!("{e:?}")))?;
        let host = cx
            .server_host()
            .map_err(|e| PayloadHandleError::Failed(format!("{e:?}")))?;
        if super::world::is_open() {
            for s in self.0 {
                if s.client_message(host, player, &channel, &data) {
                    break;
                }
            }
        }
        Ok(())
    }
}

/// The client's receiver.
struct FromServer(&'static [&'static dyn Subsystem]);

impl IPayloadHandler for FromServer {
    fn handle(
        &self,
        payload: PayloadBytes,
        cx: &mut dyn IPayloadContext,
    ) -> Result<(), PayloadHandleError> {
        let Some((channel, data)) = decode(&payload.0) else {
            return Err(PayloadHandleError::Failed(
                "malformed create:net message".into(),
            ));
        };
        let host = cx
            .client_host()
            .map_err(|e| PayloadHandleError::Failed(format!("{e:?}")))?;
        for s in self.0 {
            if s.server_message(host, &channel, &data) {
                break;
            }
        }
        Ok(())
    }
}

/// Declares `create:net` with its two receivers.
pub fn register(r: &mut dyn Registrar, systems: &'static [&'static dyn Subsystem]) {
    r.payload_handlers().registrar(VERSION).play_bidirectional(
        id(),
        Arc::new(FromClient(systems)),
        Some(Arc::new(FromServer(systems))),
    );
}

thread_local! {
    /// What waits for each player, oldest first.
    static OUTBOX: RefCell<HashMap<u128, VecDeque<PayloadBytes>>> = RefCell::new(HashMap::new());
}

fn try_send(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    bytes: PayloadBytes,
) -> Result<(), PayloadBytes> {
    let kept = PayloadBytes(bytes.0.clone());
    match player.send_payload(host, &id(), bytes) {
        Err(PayloadSendError::Backpressure) => Err(kept),
        // A player who left, or a client without Create, just misses it.
        _ => Ok(()),
    }
}

/// Sends `data` on `channel` to `player`'s client, after whatever already waits for them.
pub fn send_to_player(host: &mut dyn ServerHost, player: PlayerRef, channel: &str, data: &[u8]) {
    let bytes = encode(channel, data);
    let key = player.uuid();
    let waiting = OUTBOX.with_borrow(|o| o.get(&key).is_some_and(|q| !q.is_empty()));
    if waiting {
        OUTBOX.with_borrow_mut(|o| o.entry(key).or_default().push_back(bytes));
        return;
    }
    if let Err(kept) = try_send(host, player, bytes) {
        OUTBOX.with_borrow_mut(|o| o.entry(key).or_default().push_back(kept));
    }
}

/// Sends `data` on `channel` to each of `players`.
pub fn send_to_players(
    host: &mut dyn ServerHost,
    players: &[PlayerRef],
    channel: &str,
    data: &[u8],
) {
    for &p in players {
        send_to_player(host, p, channel, data);
    }
}

/// Sends what waits, in order, as far as each connection takes it.
pub fn flush(host: &mut dyn ServerHost) {
    if OUTBOX.with_borrow(HashMap::is_empty) {
        return;
    }
    let mut outbox = OUTBOX.with_borrow_mut(std::mem::take);
    outbox.retain(|&uuid, queue| {
        let Some(player) = host.player(uuid) else {
            return false;
        };
        while let Some(bytes) = queue.pop_front() {
            if let Err(kept) = try_send(host, player, bytes) {
                queue.push_front(kept);
                return true;
            }
        }
        false
    });
    OUTBOX.with_borrow_mut(|o| {
        // Sends made while flushing queued behind what was waiting.
        for (k, mut q) in std::mem::take(o) {
            outbox.entry(k).or_default().append(&mut q);
        }
        *o = outbox;
    });
}

/// Forgets what waits (the world closed).
pub fn clear() {
    OUTBOX.with_borrow_mut(HashMap::clear);
}

/// Sends `data` on `channel` to the server, from the client. False when it was not taken.
pub fn send_to_server(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    host.send_payload_to_server(&id(), encode(channel, data))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn messages_keep_their_channel_and_bytes() {
        let bytes = encode("create:depot", &[1, 2, 3]);
        assert_eq!(
            decode(&bytes.0),
            Some(("create:depot".to_owned(), vec![1, 2, 3]))
        );
        assert_eq!(decode(&[0xff]), None);
    }
}
