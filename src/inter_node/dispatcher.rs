use super::address::Address;
use super::decoder::{decode_message_for, encode_result_for};
use super::envelope::{InterNodeMessage, InterNodeResponse, ResponseOutcome, Topic};
use crate::{ActorError, ActorSystem};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::oneshot;
use xanq::client::Client;
use xanq::consumer::Consumer;

/// Wall-clock cap for the initial TCP `Client::connect` to the broker.
/// Bypasses the OS-default TCP connect timeout (minutes) so a missing /
/// unreachable broker fails the `ActorSystem::new` call promptly.
pub const DEFAULT_BROKER_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<ResponseOutcome>>>>;

#[derive(Clone)]
pub struct InterNodeRuntime {
    node_name: String,
    client: Arc<Client<Topic>>,
    pending: PendingMap,
    next_req_id: Arc<AtomicU64>,
}

impl InterNodeRuntime {
    /// Connect to a xanq broker using `DEFAULT_BROKER_CONNECT_TIMEOUT`.
    pub async fn connect(node_name: String, broker_addr: String) -> Result<Self, ActorError> {
        Self::connect_with_timeout(node_name, broker_addr, DEFAULT_BROKER_CONNECT_TIMEOUT).await
    }

    /// Connect with an explicit timeout. Useful in tests or when running on
    /// a slow link where 5 s isn't enough (or when you want to fail faster).
    pub async fn connect_with_timeout(
        node_name: String,
        broker_addr: String,
        timeout: Duration,
    ) -> Result<Self, ActorError> {
        let connect_fut = Client::<Topic>::connect(broker_addr.as_str());
        let client = match tokio::time::timeout(timeout, connect_fut).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(ActorError::InterNodeIo(e.to_string())),
            Err(_) => {
                return Err(ActorError::InterNodeIo(format!(
                    "broker connect to {broker_addr} timed out after {timeout:?}"
                )));
            }
        };
        Ok(Self {
            node_name,
            client: Arc::new(client),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_req_id: Arc::new(AtomicU64::new(1)),
        })
    }

    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Send a fire-and-forget envelope to `target.node`.
    pub async fn fire(
        &self,
        target: &Address,
        actor_type: &str,
        payload: Vec<u8>,
    ) -> Result<(), ActorError> {
        let envelope = InterNodeMessage::Fire {
            actor_type: actor_type.to_string(),
            target_name: target.name.clone(),
            payload,
        };
        self.client
            .produce(&Topic::request(&target.node), envelope)
            .await
            .map_err(|e| ActorError::InterNodeIo(e.to_string()))
    }

    /// Send a request envelope and wait for the matching response bytes.
    pub async fn call(
        &self,
        target: &Address,
        actor_type: &str,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, ActorError> {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self
                .pending
                .lock()
                .map_err(|_| ActorError::InterNodeRemote("pending map poisoned".into()))?;
            map.insert(req_id, tx);
        }
        let envelope = InterNodeMessage::Call {
            actor_type: actor_type.to_string(),
            target_name: target.name.clone(),
            reply_to: Topic::response(&self.node_name),
            req_id,
            payload,
        };
        if let Err(e) = self
            .client
            .produce(&Topic::request(&target.node), envelope)
            .await
        {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&req_id);
            }
            return Err(ActorError::InterNodeIo(e.to_string()));
        }
        let outcome = rx
            .await
            .map_err(|_| ActorError::InterNodeRemote("response channel dropped".into()))?;
        match outcome {
            ResponseOutcome::Ok(bytes) => Ok(bytes),
            ResponseOutcome::Err(msg) => Err(ActorError::InterNodeRemote(msg)),
        }
    }

    /// Ask `target_node` to fan out a fire-and-forget broadcast across its
    /// local actors of `actor_type` whose name matches `name_regex`.
    pub async fn broadcast_fire(
        &self,
        target_node: &str,
        actor_type: &str,
        name_regex: &str,
        payload: Vec<u8>,
    ) -> Result<(), ActorError> {
        let envelope = InterNodeMessage::BroadcastFire {
            actor_type: actor_type.to_string(),
            name_regex: name_regex.to_string(),
            payload,
        };
        self.client
            .produce(&Topic::request(target_node), envelope)
            .await
            .map_err(|e| ActorError::InterNodeIo(e.to_string()))
    }

    /// Spawn the request and response consume loops bound to this node's topics.
    pub async fn start_consumers(&self, system: ActorSystem) -> Result<(), ActorError> {
        let req_consumer = self
            .client
            .consumer::<InterNodeMessage>(&Topic::request(&self.node_name))
            .await
            .map_err(|e| ActorError::InterNodeIo(e.to_string()))?;
        let resp_consumer = self
            .client
            .consumer::<InterNodeResponse>(&Topic::response(&self.node_name))
            .await
            .map_err(|e| ActorError::InterNodeIo(e.to_string()))?;

        let rt_for_req = self.clone();
        tokio::spawn(async move {
            loop {
                match req_consumer.consume().await {
                    Ok(Some(msg)) => {
                        let rt = rt_for_req.clone();
                        let system = system.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_incoming_request(rt, system, msg).await {
                                error!("inter-node request handling failed: {:?}", e);
                            }
                        });
                    }
                    Ok(None) => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Err(e) => {
                        error!("inter-node request consume failed: {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            }
        });

        let pending = self.pending.clone();
        tokio::spawn(async move {
            loop {
                match resp_consumer.consume().await {
                    Ok(Some(resp)) => {
                        let waiter = pending.lock().ok().and_then(|mut m| m.remove(&resp.req_id));
                        if let Some(tx) = waiter {
                            let _ = tx.send(resp.outcome);
                        } else {
                            warn!("inter-node response for unknown req_id={}", resp.req_id);
                        }
                    }
                    Ok(None) => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Err(e) => {
                        error!("inter-node response consume failed: {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            }
        });

        Ok(())
    }
}

async fn handle_incoming_request(
    rt: InterNodeRuntime,
    system: ActorSystem,
    msg: InterNodeMessage,
) -> Result<(), ActorError> {
    match msg {
        InterNodeMessage::Fire {
            actor_type,
            target_name,
            payload,
        } => {
            let any_msg = decode_message_for(&actor_type, &payload)?;
            system
                .dispatch_local_any(actor_type, target_name, any_msg)
                .await
        }
        InterNodeMessage::Call {
            actor_type,
            target_name,
            reply_to,
            req_id,
            payload,
        } => {
            let any_msg = decode_message_for(&actor_type, &payload)?;
            let result = system
                .dispatch_local_any_and_recv(actor_type.clone(), target_name, any_msg)
                .await;
            let outcome = match result {
                Ok(any_result) => match encode_result_for(&actor_type, any_result) {
                    Ok(bytes) => ResponseOutcome::Ok(bytes),
                    Err(e) => ResponseOutcome::Err(e.to_string()),
                },
                Err(e) => ResponseOutcome::Err(e.to_string()),
            };
            let response = InterNodeResponse { req_id, outcome };
            rt.client
                .produce(&reply_to, response)
                .await
                .map_err(|e| ActorError::InterNodeIo(e.to_string()))?;
            Ok(())
        }
        InterNodeMessage::BroadcastFire {
            actor_type,
            name_regex,
            payload,
        } => {
            // Receiving side of a cross-node broadcast: match the regex
            // against this node's local addresses and dispatch each match.
            let matches = system.filter_address(name_regex).await;
            for name in matches {
                let any_msg = match decode_message_for(&actor_type, &payload) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("inter-node broadcast decode failed: {:?}", e);
                        continue;
                    }
                };
                if let Err(e) = system
                    .dispatch_local_any(actor_type.clone(), name, any_msg)
                    .await
                {
                    debug!("inter-node broadcast dispatch failed: {:?}", e);
                }
            }
            Ok(())
        }
    }
}
