use crate::{
    Actor, ActorError, JobController, JobSpec, LifeCycle, Mailbox, MaybeCodec, RunJobResult,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(feature = "multi-node")]
use crate::inter_node::InterNodeRuntime;

/// Commands for the ActorSystem to handle various operations
/// You can send these commands to the ActorSystem's handler channel directly.
pub enum ActorSystemCmd {
    Register {
        actor_type: String,
        #[cfg(not(feature = "multi-node"))]
        address: String,
        #[cfg(feature = "multi-node")]
        address: crate::inter_node::Address,
        mailbox: Arc<dyn Mailbox>,
        restart_tx: tokio::sync::mpsc::UnboundedSender<()>,
        kill_tx: tokio::sync::mpsc::UnboundedSender<()>,
        life_cycle: LifeCycle,
        result_tx: tokio::sync::oneshot::Sender<Result<(), ActorError>>,
        is_restarted: bool,
    },
    Restart {
        address_regex: String,
    },
    Unregister {
        address_regex: String,
    },
    FilterAddress {
        address_regex: String,
        result_tx: tokio::sync::oneshot::Sender<Vec<String>>,
    },
    FindActor {
        actor_type: String,
        address: String,
        result_tx: tokio::sync::oneshot::Sender<
            Option<(Arc<dyn Mailbox>, bool)>, // mailbox, ready
        >,
    },
    SetLifeCycle {
        address: String,
        life_cycle: LifeCycle,
    },
    RegisterJob {
        job_id: String,
        controller: JobController,
    },
    FindJob {
        job_id: String,
        result_tx: tokio::sync::oneshot::Sender<Option<JobController>>,
    },
}

#[derive(Clone)]
/// The ActorSystem is the main entry point for managing actors.
/// It contains a handler channel to send commands to the actor system.
/// It's clonable so that it can be shared across different parts of the application.
pub struct ActorSystem {
    handler_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd>,
    cache: HashMap<String, (String, Arc<dyn Mailbox>)>,
    #[cfg(feature = "multi-node")]
    node_name: String,
    #[cfg(feature = "multi-node")]
    inter_node: Option<InterNodeRuntime>,
}

#[cfg(not(feature = "multi-node"))]
impl Default for ActorSystem {
    fn default() -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut me = Self {
            handler_tx,
            cache: HashMap::new(),
        };
        me.run(handler_rx);
        me
    }
}

impl ActorSystem {
    /// Creates a new ActorSystem instance.
    ///
    /// With the `multi-node` feature enabled, pass `cluster = Some((node_name, broker_addr))`
    /// to connect to a xanq broker and participate in inter-node delivery; pass `None` to
    /// run the system in single-node mode under the same feature flag.
    #[cfg(not(feature = "multi-node"))]
    pub fn new() -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut me = Self {
            handler_tx,
            cache: HashMap::new(),
        };
        me.run(handler_rx);
        me
    }

    #[cfg(feature = "multi-node")]
    pub async fn new(
        node_name: String,
        broker_addr: Option<String>,
    ) -> Result<Self, ActorError> {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::unbounded_channel();
        let inter_node = match broker_addr {
            Some(addr) => Some(InterNodeRuntime::connect(node_name.clone(), addr).await?),
            None => None,
        };
        let mut me = Self {
            handler_tx,
            cache: HashMap::new(),
            node_name,
            inter_node: inter_node.clone(),
        };
        me.run(handler_rx);
        if let Some(rt) = inter_node {
            rt.start_consumers(me.clone()).await?;
        }
        Ok(me)
    }

    /// This system's cluster node name (set during `new`).
    #[cfg(feature = "multi-node")]
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Look up a local actor by `(actor_type, address)` and forward the
    /// already-decoded payload. Used by the inter-node request consumer.
    #[cfg(feature = "multi-node")]
    pub async fn dispatch_local_any(
        &self,
        actor_type: String,
        address: String,
        payload: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), ActorError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
            actor_type,
            address: address.clone(),
            result_tx: tx,
        });
        if let Ok(Some((mailbox, ready))) = rx.await {
            if ready {
                mailbox.send(payload).await
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    #[cfg(feature = "multi-node")]
    pub async fn dispatch_local_any_and_recv(
        &self,
        actor_type: String,
        address: String,
        payload: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<Box<dyn std::any::Any + Send>, ActorError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
            actor_type,
            address: address.clone(),
            result_tx: tx,
        });
        if let Ok(Some((mailbox, ready))) = rx.await {
            if ready {
                mailbox.send_and_recv(payload).await
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    /// Inter-node variant of `run_job`. Encodes the payload once, then spawns
    /// a loop that drives the remote actor over the xanq broker on the same
    /// `JobSpec` schedule and exposes the same `JobController`.
    #[cfg(feature = "multi-node")]
    async fn spawn_remote_job<T>(
        &self,
        address: crate::inter_node::Address,
        subscribe: bool,
        job: JobSpec,
        msg: <T as Actor>::Message,
        job_id: String,
        rt: crate::inter_node::InterNodeRuntime,
    ) -> Result<RunJobResult<T>, ActorError>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
        <T as Actor>::Result: MaybeCodec,
    {
        let payload_bytes = <<T as Actor>::Message as xancode::Codec>::encode(&msg).to_vec();
        let actor_type = std::any::type_name::<T>().to_string();

        let (abort_tx, mut abort_rx) = tokio::sync::mpsc::unbounded_channel();
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
        let (resume_tx, mut resume_rx) = tokio::sync::mpsc::unbounded_channel();

        let result_subscriber_rx = if subscribe {
            let (sub_tx, sub_rx) = tokio::sync::mpsc::unbounded_channel();
            let rt = rt.clone();
            let address = address.clone();
            let actor_type = actor_type.clone();
            let payload_bytes = payload_bytes.clone();
            tokio::spawn(async move {
                let mut i = 0usize;
                loop {
                    let until_start = job
                        .start_at()
                        .duration_since(std::time::SystemTime::now())
                        .unwrap_or(std::time::Duration::ZERO);
                    if !delay_or_abort(until_start, &mut abort_rx, &mut stop_rx, &mut resume_rx)
                        .await
                    {
                        drop(sub_tx);
                        return;
                    }
                    i += 1;
                    let outcome: Result<<T as Actor>::Result, ActorError> = match rt
                        .call(&address, &actor_type, payload_bytes.clone())
                        .await
                    {
                        Ok(bytes) => <<T as Actor>::Result as xancode::Codec>::decode(
                            &xancode::Bytes::copy_from_slice(&bytes),
                        )
                        .map_err(|_| {
                            ActorError::InterNodeDecode(format!(
                                "decode failed for {}",
                                std::any::type_name::<<T as Actor>::Result>()
                            ))
                        }),
                        Err(e) => Err(e),
                    };
                    let _ = sub_tx.send(outcome);
                    if let Some(max_iter) = job.max_iter() {
                        if i >= max_iter {
                            drop(sub_tx);
                            return;
                        }
                    }
                    let interval = match job.interval() {
                        Some(d) => d,
                        None => {
                            drop(sub_tx);
                            return;
                        }
                    };
                    if !delay_or_abort(interval, &mut abort_rx, &mut stop_rx, &mut resume_rx).await
                    {
                        drop(sub_tx);
                        return;
                    }
                }
            });
            Some(sub_rx)
        } else {
            let rt = rt.clone();
            tokio::spawn(async move {
                let mut i = 0usize;
                loop {
                    let until_start = job
                        .start_at()
                        .duration_since(std::time::SystemTime::now())
                        .unwrap_or(std::time::Duration::ZERO);
                    if !delay_or_abort(until_start, &mut abort_rx, &mut stop_rx, &mut resume_rx)
                        .await
                    {
                        return;
                    }
                    i += 1;
                    let _ = rt
                        .fire(&address, &actor_type, payload_bytes.clone())
                        .await;
                    if let Some(max_iter) = job.max_iter() {
                        if i >= max_iter {
                            return;
                        }
                    }
                    let interval = match job.interval() {
                        Some(d) => d,
                        None => return,
                    };
                    if !delay_or_abort(interval, &mut abort_rx, &mut stop_rx, &mut resume_rx).await
                    {
                        return;
                    }
                }
            });
            None
        };

        let _ = self.handler_tx.send(ActorSystemCmd::RegisterJob {
            job_id: job_id.clone(),
            controller: JobController {
                abort_tx,
                stop_tx,
                resume_tx,
            },
        });

        Ok(RunJobResult {
            job_id,
            result_subscriber_rx,
        })
    }

    /// Returns the handler channel sender for the ActorSystem.
    /// You can use this to send commands to the ActorSystem directly.
    pub fn handler_tx(&self) -> tokio::sync::mpsc::UnboundedSender<ActorSystemCmd> {
        self.handler_tx.clone()
    }

    /// Filters the addresses of actors based on a regex pattern.
    pub async fn filter_address(&self, address_regex: String) -> Vec<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::FilterAddress {
            address_regex,
            result_tx: tx,
        });
        match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                Vec::new()
            }
        }
    }

    /// Restarts an actor.
    pub fn restart(&mut self, address_regex: String) {
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::Restart { address_regex });
    }

    /// Unregisters an actor.
    pub fn unregister(&mut self, address_regex: String) {
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::Unregister { address_regex });
    }

    /// Send a message to a specific actor by its address.
    /// It doesn't wait for the actor to be ready.
    pub async fn send<T>(
        &mut self,
        #[cfg(not(feature = "multi-node"))] address: String,
        #[cfg(feature = "multi-node")] address: crate::inter_node::Address,
        msg: <T as Actor>::Message,
    ) -> Result<(), ActorError>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
    {
        #[cfg(feature = "multi-node")]
        if address.node != self.node_name {
            let rt = self
                .inter_node
                .as_ref()
                .ok_or(ActorError::InterNodeNotConfigured)?
                .clone();
            let payload = <<T as Actor>::Message as xancode::Codec>::encode(&msg).to_vec();
            return rt
                .fire(&address, std::any::type_name::<T>(), payload)
                .await;
        }
        #[cfg(feature = "multi-node")]
        let address: String = address.name;

        let mut retry_count = 0;
        let actor_type = std::any::type_name::<T>();
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            match self.cache.entry(address.clone()) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    let (cached_actor_type, mailbox) = o.get().clone();
                    if actor_type == cached_actor_type {
                        match mailbox.send(payload.clone()).await {
                            Ok(()) => {
                                debug!(
                                    "Send message to actor {} through cached_tx succeeded",
                                    address
                                );
                                return Ok(());
                            }
                            Err(e) => {
                                warn!(
                                    "Send message to actor {} through cached_tx failed: {:?} ... removing from cache",
                                    address, e
                                );
                                self.cache.remove(&address);
                            }
                        }
                    } else {
                        warn!(
                            "Send message with cached tx failed: cached tx of address {} and target actor {} is mismatched ... removing from cache",
                            address, actor_type,
                        );
                        self.cache.remove(&address);
                    }
                }
                _ => {}
            }
            let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
                actor_type: actor_type.to_string(),
                address: address.clone(),
                result_tx: tx,
            });
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    debug!("Saving actor {} tx to cache", address);
                    self.cache
                        .insert(address.clone(), (actor_type.to_string(), tx.clone()));
                    let _ = tx.send(payload.clone()).await?;
                    return Ok(());
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address));
            }
        }
    }

    /// Send a message to a specific actor by its address.
    /// It doesn't wait for the actor to be ready.
    /// It doesn't cache the actor's tx for future use.
    pub async fn send_without_tx_cache<T>(
        &self,
        #[cfg(not(feature = "multi-node"))] address: String,
        #[cfg(feature = "multi-node")] address: crate::inter_node::Address,
        msg: <T as Actor>::Message,
    ) -> Result<(), ActorError>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
    {
        #[cfg(feature = "multi-node")]
        if address.node != self.node_name {
            let rt = self
                .inter_node
                .as_ref()
                .ok_or(ActorError::InterNodeNotConfigured)?
                .clone();
            let payload = <<T as Actor>::Message as xancode::Codec>::encode(&msg).to_vec();
            return rt
                .fire(&address, std::any::type_name::<T>(), payload)
                .await;
        }
        #[cfg(feature = "multi-node")]
        let address: String = address.name;

        let mut retry_count = 0;
        let actor_type = std::any::type_name::<T>();
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
                actor_type: actor_type.to_string(),
                address: address.clone(),
                result_tx: tx,
            });
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    let _ = tx.send(payload.clone()).await?;
                    return Ok(());
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address));
            }
        }
    }

    /// Local regex fan-out helper (cached). One entry per matched local actor.
    async fn broadcast_local_cached<T>(
        &mut self,
        address_regex: String,
        msg: <T as Actor>::Message,
    ) -> Vec<Result<(), ActorError>>
    where
        T: Actor,
    {
        let actor_type = std::any::type_name::<T>();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::FilterAddress {
            address_regex,
            result_tx: tx,
        });
        let addresses = match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                return vec![Err(ActorError::from(e))];
            }
        };
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let mut result = Vec::new();
        for address in addresses.iter() {
            match self.cache.entry(address.clone()) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    let (cached_actor_type, tx) = o.get().clone();
                    if cached_actor_type == actor_type {
                        match tx.send(payload.clone()).await {
                            Ok(()) => {
                                result.push(Ok(()));
                                continue;
                            }
                            Err(e) => {
                                warn!(
                                    "Send message to actor {} through cached_tx failed: {:?} ... removing from cache",
                                    address, e
                                );
                                self.cache.remove(address);
                            }
                        }
                    } else {
                        warn!(
                            "Send message with cached tx failed: cached tx of address {} and target actor {} is mismatched ... removing from cache",
                            address, actor_type,
                        );
                        self.cache.remove(address);
                    }
                }
                _ => {}
            }
            let mut retry_count = 0;
            loop {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                });
                if let Ok(Some((tx, ready))) = rx.await {
                    if ready {
                        self.cache
                            .insert(address.clone(), (actor_type.to_string(), tx.clone()));
                        result.push(tx.send(payload.clone()).await);
                        break;
                    } else {
                        retry_count += 1;
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        if retry_count < 10 {
                            continue;
                        } else {
                            error!("Actor {} not ready after 10 retries, giving up", address);
                            result.push(Err(ActorError::ActorNotReady(address.clone())));
                            break;
                        }
                    }
                } else {
                    result.push(Err(ActorError::AddressNotFound(address.clone())));
                    break;
                }
            }
        }
        result
    }

    /// Sends a message to all actors that match the given address regex.
    /// Returns one entry per matched local actor.
    #[cfg(not(feature = "multi-node"))]
    pub async fn send_broadcast<T>(
        &mut self,
        address_regex: String,
        msg: <T as Actor>::Message,
    ) -> Vec<Result<(), ActorError>>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
    {
        self.broadcast_local_cached::<T>(address_regex, msg).await
    }

    /// Sends a message across `filter`'s nodes. `BroadcastResult` separates
    /// the local per-actor results from the remote per-peer envelope acks.
    #[cfg(feature = "multi-node")]
    pub async fn send_broadcast<T>(
        &mut self,
        address_regex: String,
        filter: crate::inter_node::NodeFilter,
        msg: <T as Actor>::Message,
    ) -> crate::inter_node::BroadcastResult
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
    {
        use crate::inter_node::{BroadcastResult, NodeFilter};
        use std::collections::HashSet;

        let (run_local, remote_nodes): (bool, Vec<String>) = {
            let raw: Vec<String> = match filter {
                NodeFilter::SelfOnly => vec![self.node_name.clone()],
                NodeFilter::Node(n) => vec![n],
                NodeFilter::Peers(ns) => ns,
            };
            let mut seen = HashSet::new();
            let mut local = false;
            let mut remote = Vec::new();
            for node in raw {
                if !seen.insert(node.clone()) {
                    continue;
                }
                if node == self.node_name {
                    local = true;
                } else {
                    remote.push(node);
                }
            }
            (local, remote)
        };

        let remote: Vec<Result<(), ActorError>> = if !remote_nodes.is_empty() {
            let rt = match self.inter_node.as_ref() {
                Some(rt) => rt.clone(),
                None => {
                    return BroadcastResult {
                        local: Vec::new(),
                        remote: vec![Err(ActorError::InterNodeNotConfigured)],
                    };
                }
            };
            let bytes = <<T as Actor>::Message as xancode::Codec>::encode(&msg).to_vec();
            let actor_type = std::any::type_name::<T>();
            let mut out = Vec::with_capacity(remote_nodes.len());
            for node in &remote_nodes {
                out.push(
                    rt.broadcast_fire(node, actor_type, &address_regex, bytes.clone())
                        .await,
                );
            }
            out
        } else {
            Vec::new()
        };

        let local = if run_local {
            self.broadcast_local_cached::<T>(address_regex, msg).await
        } else {
            Vec::new()
        };

        BroadcastResult { local, remote }
    }

    /// Sends a message to all actors that match the given address regex.
    /// It returns a vector of results, success or error for each actor.
    /// It doesn't returns results from the actors, only whether the message was sent successfully or not.
    /// It doesn't cache the actor's tx for future use.
    /// Local regex fan-out helper (no cache).
    async fn broadcast_local_uncached<T>(
        &self,
        address_regex: String,
        msg: <T as Actor>::Message,
    ) -> Vec<Result<(), ActorError>>
    where
        T: Actor,
    {
        let actor_type = std::any::type_name::<T>();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::FilterAddress {
            address_regex,
            result_tx: tx,
        });
        let addresses = match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                return vec![Err(ActorError::from(e))];
            }
        };
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let mut result = Vec::new();
        for address in addresses.iter() {
            let mut retry_count = 0;
            loop {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                });
                if let Ok(Some((tx, ready))) = rx.await {
                    if ready {
                        result.push(tx.send(payload.clone()).await);
                        break;
                    } else {
                        retry_count += 1;
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        if retry_count < 10 {
                            continue;
                        } else {
                            error!("Actor {} not ready after 10 retries, giving up", address);
                            result.push(Err(ActorError::ActorNotReady(address.clone())));
                            break;
                        }
                    }
                } else {
                    result.push(Err(ActorError::AddressNotFound(address.clone())));
                    break;
                }
            }
        }
        result
    }

    /// Non-cached version of `send_broadcast`. Same return-shape rules apply.
    #[cfg(not(feature = "multi-node"))]
    pub async fn send_broadcast_without_tx_cache<T>(
        &self,
        address_regex: String,
        msg: <T as Actor>::Message,
    ) -> Vec<Result<(), ActorError>>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
    {
        self.broadcast_local_uncached::<T>(address_regex, msg).await
    }

    #[cfg(feature = "multi-node")]
    pub async fn send_broadcast_without_tx_cache<T>(
        &self,
        address_regex: String,
        filter: crate::inter_node::NodeFilter,
        msg: <T as Actor>::Message,
    ) -> crate::inter_node::BroadcastResult
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
    {
        use crate::inter_node::{BroadcastResult, NodeFilter};
        use std::collections::HashSet;

        let (run_local, remote_nodes): (bool, Vec<String>) = {
            let raw: Vec<String> = match filter {
                NodeFilter::SelfOnly => vec![self.node_name.clone()],
                NodeFilter::Node(n) => vec![n],
                NodeFilter::Peers(ns) => ns,
            };
            let mut seen = HashSet::new();
            let mut local = false;
            let mut remote = Vec::new();
            for node in raw {
                if !seen.insert(node.clone()) {
                    continue;
                }
                if node == self.node_name {
                    local = true;
                } else {
                    remote.push(node);
                }
            }
            (local, remote)
        };

        let remote: Vec<Result<(), ActorError>> = if !remote_nodes.is_empty() {
            let rt = match self.inter_node.as_ref() {
                Some(rt) => rt.clone(),
                None => {
                    return BroadcastResult {
                        local: Vec::new(),
                        remote: vec![Err(ActorError::InterNodeNotConfigured)],
                    };
                }
            };
            let bytes = <<T as Actor>::Message as xancode::Codec>::encode(&msg).to_vec();
            let actor_type = std::any::type_name::<T>();
            let mut out = Vec::with_capacity(remote_nodes.len());
            for node in &remote_nodes {
                out.push(
                    rt.broadcast_fire(node, actor_type, &address_regex, bytes.clone())
                        .await,
                );
            }
            out
        } else {
            Vec::new()
        };

        let local = if run_local {
            self.broadcast_local_uncached::<T>(address_regex, msg).await
        } else {
            Vec::new()
        };

        BroadcastResult { local, remote }
    }

    /// Sends a message to a specific actor and waits for the result.
    pub async fn send_and_recv<T>(
        &mut self,
        #[cfg(not(feature = "multi-node"))] address: String,
        #[cfg(feature = "multi-node")] address: crate::inter_node::Address,
        msg: <T as Actor>::Message,
    ) -> Result<<T as Actor>::Result, ActorError>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
        <T as Actor>::Result: MaybeCodec,
    {
        #[cfg(feature = "multi-node")]
        if address.node != self.node_name {
            let rt = self
                .inter_node
                .as_ref()
                .ok_or(ActorError::InterNodeNotConfigured)?
                .clone();
            let payload = <<T as Actor>::Message as xancode::Codec>::encode(&msg).to_vec();
            let bytes = rt
                .call(&address, std::any::type_name::<T>(), payload)
                .await?;
            let result = <<T as Actor>::Result as xancode::Codec>::decode(
                &xancode::Bytes::copy_from_slice(&bytes),
            )
            .map_err(|_| {
                ActorError::InterNodeDecode(format!(
                    "decode failed for {}",
                    std::any::type_name::<<T as Actor>::Result>()
                ))
            })?;
            return Ok(result);
        }
        #[cfg(feature = "multi-node")]
        let address: String = address.name;

        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let actor_type = std::any::type_name::<T>();
        let mut retry_count = 0;
        loop {
            match self.cache.entry(address.clone()) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    let (cached_actor_type, tx) = o.get().clone();
                    if cached_actor_type == actor_type {
                        match tx.send_and_recv(payload.clone()).await {
                            Ok(result_any) => {
                                debug!(
                                    "Send message to actor {} through cached_tx succeeded",
                                    address
                                );
                                let result = result_any
                                    .downcast::<T::Result>()
                                    .map_err(|_| ActorError::MessageTypeMismatch)?;
                                return Ok(*result);
                            }
                            Err(e) => {
                                warn!(
                                    "Send message to actor {} through cached_tx failed: {:?} ... removing from cache",
                                    address, e
                                );
                                self.cache.remove(&address);
                            }
                        }
                    } else {
                        warn!(
                            "Send message with cached tx failed: cached tx of address {} and target actor {} is mismatched ... removing from cache",
                            address, actor_type,
                        );
                        self.cache.remove(&address);
                    }
                }
                _ => {}
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
                actor_type: actor_type.to_string(),
                address: address.clone(),
                result_tx: tx,
            });
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    debug!("Saving actor {} tx to cache", address);
                    self.cache
                        .insert(address.clone(), (actor_type.to_string(), tx.clone()));
                    let result_any = tx.send_and_recv(payload.clone()).await?;
                    let result = result_any
                        .downcast::<T::Result>()
                        .map_err(|_| ActorError::MessageTypeMismatch)?;
                    return Ok(*result);
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address));
            }
        }
    }

    /// Sends a message to a specific actor and waits for the result.
    /// It doesn't cache the actor's tx for future use.
    pub async fn send_and_recv_without_tx_cache<T>(
        &self,
        #[cfg(not(feature = "multi-node"))] address: String,
        #[cfg(feature = "multi-node")] address: crate::inter_node::Address,
        msg: <T as Actor>::Message,
    ) -> Result<<T as Actor>::Result, ActorError>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
        <T as Actor>::Result: MaybeCodec,
    {
        #[cfg(feature = "multi-node")]
        if address.node != self.node_name {
            let rt = self
                .inter_node
                .as_ref()
                .ok_or(ActorError::InterNodeNotConfigured)?
                .clone();
            let payload = <<T as Actor>::Message as xancode::Codec>::encode(&msg).to_vec();
            let bytes = rt
                .call(&address, std::any::type_name::<T>(), payload)
                .await?;
            let result = <<T as Actor>::Result as xancode::Codec>::decode(
                &xancode::Bytes::copy_from_slice(&bytes),
            )
            .map_err(|_| {
                ActorError::InterNodeDecode(format!(
                    "decode failed for {}",
                    std::any::type_name::<<T as Actor>::Result>()
                ))
            })?;
            return Ok(result);
        }
        #[cfg(feature = "multi-node")]
        let address: String = address.name;

        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let actor_type = std::any::type_name::<T>();
        let mut retry_count = 0;
        loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
                actor_type: actor_type.to_string(),
                address: address.clone(),
                result_tx: tx,
            });
            if let Ok(Some((tx, ready))) = rx.await {
                if ready {
                    let result_any = tx.send_and_recv(payload.clone()).await?;
                    let result = result_any
                        .downcast::<T::Result>()
                        .map_err(|_| ActorError::MessageTypeMismatch)?;
                    return Ok(*result);
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address));
            }
        }
    }

    /// Runs a job with the specified actor and message.
    /// If you want to subscribe to the results, set `subscribe` to true.
    /// It returns a receiver that you can use to receive the results.
    /// If you want to set a job_id, set it to `Some(job_id)`.
    pub async fn run_job<T>(
        &mut self,
        #[cfg(not(feature = "multi-node"))] address: String,
        #[cfg(feature = "multi-node")] address: crate::inter_node::Address,
        subscribe: bool,
        job: JobSpec,
        msg: <T as Actor>::Message,
        job_id: Option<String>,
    ) -> Result<RunJobResult<T>, ActorError>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
        <T as Actor>::Result: MaybeCodec,
    {
        let job_id = job_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        #[cfg(feature = "multi-node")]
        if address.node != self.node_name {
            let rt = self
                .inter_node
                .as_ref()
                .ok_or(ActorError::InterNodeNotConfigured)?
                .clone();
            return self
                .spawn_remote_job::<T>(address, subscribe, job, msg, job_id, rt)
                .await;
        }
        #[cfg(feature = "multi-node")]
        let address: String = address.name;

        let mut retry_count = 0;
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let actor_type = std::any::type_name::<T>();
        let mailbox = loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
                actor_type: actor_type.to_string(),
                address: address.clone(),
                result_tx: tx,
            });
            if let Ok(Some((mailbox, ready))) = rx.await {
                if ready {
                    debug!("Saving actor {} tx to cache", address);
                    if let None = self.cache.get(&address) {
                        self.cache
                            .insert(address.clone(), (actor_type.to_string(), mailbox.clone()));
                    }
                    break mailbox;
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address.clone()));
            }
        };
        if subscribe {
            let (sub_tx, sub_rx) = tokio::sync::mpsc::unbounded_channel();
            let (abort_tx, abort_rx) = tokio::sync::mpsc::unbounded_channel();
            let (stop_tx, stop_rx) = tokio::sync::mpsc::unbounded_channel();
            let (resume_tx, resume_rx) = tokio::sync::mpsc::unbounded_channel();
            let payload = payload.clone();
            let _ = tokio::spawn(async move {
                let mut i = 0usize;
                let mut abort_rx = abort_rx;
                let mut stop_rx = stop_rx;
                let mut resume_rx = resume_rx;
                loop {
                    let until_start = job
                        .start_at()
                        .duration_since(std::time::SystemTime::now())
                        .unwrap_or(std::time::Duration::ZERO);
                    if !delay_or_abort(until_start, &mut abort_rx, &mut stop_rx, &mut resume_rx)
                        .await
                    {
                        drop(sub_tx);
                        return;
                    }
                    i += 1;
                    let result = match mailbox.send_and_recv(payload.clone()).await {
                        Ok(result_any) => result_any
                            .downcast::<T::Result>()
                            .map(|x| Ok(*x))
                            .unwrap_or_else(|_| Err(ActorError::MessageTypeMismatch)),
                        Err(e) => Err(e),
                    };
                    let _ = sub_tx.send(result);
                    if let Some(max_iter) = job.max_iter() {
                        if i >= max_iter {
                            drop(sub_tx);
                            return;
                        }
                    }
                    let interval = match job.interval() {
                        Some(d) => d,
                        None => {
                            drop(sub_tx);
                            return;
                        }
                    };
                    if !delay_or_abort(interval, &mut abort_rx, &mut stop_rx, &mut resume_rx).await
                    {
                        drop(sub_tx);
                        return;
                    }
                }
            });
            let _ = self.handler_tx.send(ActorSystemCmd::RegisterJob {
                job_id: job_id.clone(),
                controller: JobController {
                    abort_tx,
                    stop_tx,
                    resume_tx,
                },
            });
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: Some(sub_rx),
            });
        } else {
            let (abort_tx, abort_rx) = tokio::sync::mpsc::unbounded_channel();
            let (stop_tx, stop_rx) = tokio::sync::mpsc::unbounded_channel();
            let (resume_tx, resume_rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tokio::spawn(async move {
                let mut i = 0usize;
                let mut abort_rx = abort_rx;
                let mut stop_rx = stop_rx;
                let mut resume_rx = resume_rx;
                loop {
                    let until_start = job
                        .start_at()
                        .duration_since(std::time::SystemTime::now())
                        .unwrap_or(std::time::Duration::ZERO);
                    if !delay_or_abort(until_start, &mut abort_rx, &mut stop_rx, &mut resume_rx)
                        .await
                    {
                        return;
                    }
                    i += 1;
                    let _ = mailbox.send(payload.clone()).await;
                    if let Some(max_iter) = job.max_iter() {
                        if i >= max_iter {
                            return;
                        }
                    }
                    let interval = match job.interval() {
                        Some(d) => d,
                        None => return,
                    };
                    if !delay_or_abort(interval, &mut abort_rx, &mut stop_rx, &mut resume_rx).await
                    {
                        return;
                    }
                }
            });
            let _ = self.handler_tx.send(ActorSystemCmd::RegisterJob {
                job_id: job_id.clone(),
                controller: JobController {
                    abort_tx,
                    stop_tx,
                    resume_tx,
                },
            });
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: None,
            });
        }
    }

    /// Runs a job with the specified actor and message.
    /// If you want to subscribe to the results, set `subscribe` to true.
    /// It returns a receiver that you can use to receive the results.
    /// It doesn't cache the actor's tx for future use.
    /// If you want to set a job_id, set it to `Some(job_id)`.
    pub async fn run_job_without_tx_cache<T>(
        &self,
        #[cfg(not(feature = "multi-node"))] address: String,
        #[cfg(feature = "multi-node")] address: crate::inter_node::Address,
        subscribe: bool,
        job: JobSpec,
        msg: <T as Actor>::Message,
        job_id: Option<String>,
    ) -> Result<RunJobResult<T>, ActorError>
    where
        T: Actor,
        <T as Actor>::Message: MaybeCodec,
        <T as Actor>::Result: MaybeCodec,
    {
        let job_id = job_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        #[cfg(feature = "multi-node")]
        if address.node != self.node_name {
            let rt = self
                .inter_node
                .as_ref()
                .ok_or(ActorError::InterNodeNotConfigured)?
                .clone();
            return self
                .spawn_remote_job::<T>(address, subscribe, job, msg, job_id, rt)
                .await;
        }
        #[cfg(feature = "multi-node")]
        let address: String = address.name;

        let mut retry_count = 0;
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(msg);
        let actor_type = std::any::type_name::<T>();
        let mailbox = loop {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = self.handler_tx.send(ActorSystemCmd::FindActor {
                actor_type: actor_type.to_string(),
                address: address.clone(),
                result_tx: tx,
            });
            if let Ok(Some((mailbox, ready))) = rx.await {
                if ready {
                    break mailbox;
                } else {
                    retry_count += 1;
                    debug!(
                        "Actor {} not ready, retrying... ({}/10)",
                        address, retry_count
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if retry_count < 10 {
                        continue;
                    } else {
                        error!("Actor {} not ready after 10 retries, giving up", address);
                        return Err(ActorError::ActorNotReady(address));
                    }
                }
            } else {
                return Err(ActorError::AddressNotFound(address.clone()));
            }
        };
        if subscribe {
            let (sub_tx, sub_rx) = tokio::sync::mpsc::unbounded_channel();
            let payload = payload.clone();
            let (abort_tx, abort_rx) = tokio::sync::mpsc::unbounded_channel();
            let (stop_tx, stop_rx) = tokio::sync::mpsc::unbounded_channel();
            let (resume_tx, resume_rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tokio::spawn(async move {
                let mut i = 0usize;
                let mut abort_rx = abort_rx;
                let mut stop_rx = stop_rx;
                let mut resume_rx = resume_rx;
                loop {
                    let until_start = job
                        .start_at()
                        .duration_since(std::time::SystemTime::now())
                        .unwrap_or(std::time::Duration::ZERO);
                    if !delay_or_abort(until_start, &mut abort_rx, &mut stop_rx, &mut resume_rx)
                        .await
                    {
                        drop(sub_tx);
                        return;
                    }
                    i += 1;
                    let result = match mailbox.send_and_recv(payload.clone()).await {
                        Ok(result_any) => result_any
                            .downcast::<T::Result>()
                            .map(|x| Ok(*x))
                            .unwrap_or_else(|_| Err(ActorError::MessageTypeMismatch)),
                        Err(e) => Err(e),
                    };
                    let _ = sub_tx.send(result);
                    if let Some(max_iter) = job.max_iter() {
                        if i >= max_iter {
                            drop(sub_tx);
                            return;
                        }
                    }
                    let interval = match job.interval() {
                        Some(d) => d,
                        None => {
                            drop(sub_tx);
                            return;
                        }
                    };
                    if !delay_or_abort(interval, &mut abort_rx, &mut stop_rx, &mut resume_rx).await
                    {
                        drop(sub_tx);
                        return;
                    }
                }
            });
            let _ = self.handler_tx.send(ActorSystemCmd::RegisterJob {
                job_id: job_id.clone(),
                controller: JobController {
                    abort_tx,
                    stop_tx,
                    resume_tx,
                },
            });
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: Some(sub_rx),
            });
        } else {
            let (abort_tx, abort_rx) = tokio::sync::mpsc::unbounded_channel();
            let (stop_tx, stop_rx) = tokio::sync::mpsc::unbounded_channel();
            let (resume_tx, resume_rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tokio::spawn(async move {
                let mut i = 0usize;
                let mut abort_rx = abort_rx;
                let mut stop_rx = stop_rx;
                let mut resume_rx = resume_rx;
                loop {
                    let until_start = job
                        .start_at()
                        .duration_since(std::time::SystemTime::now())
                        .unwrap_or(std::time::Duration::ZERO);
                    if !delay_or_abort(until_start, &mut abort_rx, &mut stop_rx, &mut resume_rx)
                        .await
                    {
                        return;
                    }
                    i += 1;
                    let _ = mailbox.send(payload.clone()).await;
                    if let Some(max_iter) = job.max_iter() {
                        if i >= max_iter {
                            return;
                        }
                    }
                    let interval = match job.interval() {
                        Some(d) => d,
                        None => return,
                    };
                    if !delay_or_abort(interval, &mut abort_rx, &mut stop_rx, &mut resume_rx).await
                    {
                        return;
                    }
                }
            });
            let _ = self.handler_tx.send(ActorSystemCmd::RegisterJob {
                job_id: job_id.clone(),
                controller: JobController {
                    abort_tx,
                    stop_tx,
                    resume_tx,
                },
            });
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: None,
            });
        }
    }

    pub async fn abort_job(&self, job_id: String) {
        info!("Aborting job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::FindJob {
            job_id: job_id.clone(),
            result_tx: tx,
        });
        match rx.await {
            Ok(Some(controller)) => {
                let _ = controller.abort_tx.send(());
            }
            Ok(None) => {
                error!("Job {} not found", job_id);
            }
            Err(e) => {
                error!("Find job {} failed: {}", job_id, e);
            }
        }
    }

    pub async fn stop_job(&self, job_id: String) {
        info!("Stopping job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::FindJob {
            job_id: job_id.clone(),
            result_tx: tx,
        });
        match rx.await {
            Ok(Some(controller)) => {
                let _ = controller.stop_tx.send(());
            }
            Ok(None) => {
                error!("Job {} not found", job_id);
            }
            Err(e) => {
                error!("Find job {} failed: {}", job_id, e);
            }
        }
    }

    pub async fn resume_job(&self, job_id: String) {
        info!("Resuming job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::FindJob {
            job_id: job_id.clone(),
            result_tx: tx,
        });
        match rx.await {
            Ok(Some(controller)) => {
                let _ = controller.resume_tx.send(());
            }
            Ok(None) => {
                error!("Job {} not found", job_id);
            }
            Err(e) => {
                error!("Find job {} failed: {}", job_id, e);
            }
        }
    }

    fn run(
        &mut self,
        handler_rx: tokio::sync::mpsc::UnboundedReceiver<ActorSystemCmd>,
    ) -> tokio::task::JoinHandle<()> {
        #[cfg(feature = "multi-node")]
        let inter_node = self.inter_node.clone();
        let handle = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(actor_system_loop(
                handler_rx,
                #[cfg(feature = "multi-node")]
                inter_node,
            ))
        });
        handle
    }
}

// {{{ fn actor_system_loop
async fn actor_system_loop(
    mut handler_rx: tokio::sync::mpsc::UnboundedReceiver<ActorSystemCmd>,
    #[cfg(feature = "multi-node")] inter_node: Option<InterNodeRuntime>,
) {
    let mut address_list = HashSet::<String>::new();
    let mut map = HashMap::<
        String,
        (
            String,
            Arc<dyn Mailbox>,
            tokio::sync::mpsc::UnboundedSender<()>,
            tokio::sync::mpsc::UnboundedSender<()>,
            LifeCycle,
        ),
    >::new();
    let mut job_controllers = HashMap::new();
    while let Some(msg) = handler_rx.recv().await {
        match msg {
            ActorSystemCmd::Register {
                actor_type,
                address,
                mailbox,
                restart_tx,
                kill_tx,
                life_cycle,
                result_tx,
                is_restarted,
            } => {
                #[cfg(not(feature = "multi-node"))]
                let name: String = address;
                #[cfg(feature = "multi-node")]
                let name: String = {
                    if let Some(rt) = inter_node.as_ref() {
                        if address.node != rt.node_name() {
                            let _ = result_tx
                                .send(Err(ActorError::AddressNotOwned(address.to_string())));
                            continue;
                        }
                    }
                    address.name
                };
                debug!(
                    "Register actor with address {} with type {}",
                    name, actor_type
                );
                if map.contains_key(&name) && !is_restarted {
                    let _ = result_tx.send(Err(ActorError::AddressAlreadyExist(name)));
                    continue;
                }
                map.insert(
                    name.clone(),
                    (actor_type, mailbox, restart_tx, kill_tx, life_cycle),
                );
                address_list.insert(name);
                let _ = result_tx.send(Ok(()));
            }
            ActorSystemCmd::Restart { address_regex } => {
                debug!("Restart actor with address {}", address_regex);
                let addresses = match filter_address(&address_list, &address_regex) {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        error!("Filter address failed: {:?}", e);
                        continue;
                    }
                };
                for address in addresses {
                    if let Some((_actor_type, _tx, restart_tx, _kill_tx, life_cycle)) =
                        map.get_mut(&address)
                    {
                        *life_cycle = LifeCycle::Restarting;
                        let _ = restart_tx.send(());
                    }
                }
            }
            ActorSystemCmd::Unregister { address_regex } => {
                debug!("Unregister actor with address {}", address_regex);
                let addresses = match filter_address(&address_list, &address_regex) {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        error!("Filter address failed: {:?}", e);
                        continue;
                    }
                };
                for address in addresses {
                    match map.entry(address.to_string()) {
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            let _ = entry.get_mut().3.send(());
                            entry.remove_entry();
                            address_list.remove(&address);
                        }
                        std::collections::hash_map::Entry::Vacant(_) => {
                            continue;
                        }
                    }
                }
            }
            ActorSystemCmd::FilterAddress {
                address_regex,
                result_tx,
            } => {
                debug!("FilterAddress with regex {}", address_regex);
                let addresses = match filter_address(&address_list, &address_regex) {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        error!("Filter address failed: {:?}", e);
                        continue;
                    }
                };
                let _ = result_tx.send(addresses);
            }
            ActorSystemCmd::FindActor {
                actor_type,
                address,
                result_tx,
            } => {
                debug!(
                    "FindActor with address {} with type {}",
                    address, actor_type
                );
                if let Some((target_actor_type, tx, _restart_tx, _kill_tx, life_cycle)) =
                    map.get(&address)
                {
                    match life_cycle {
                        LifeCycle::Receiving => {
                            if *target_actor_type == actor_type {
                                let _ = result_tx.send(Some((tx.clone(), true)));
                            } else {
                                let _ = result_tx.send(None);
                            }
                        }
                        _ => {
                            let _ = result_tx.send(Some((tx.clone(), false)));
                        }
                    }
                } else {
                    let _ = result_tx.send(None);
                }
            }
            ActorSystemCmd::SetLifeCycle {
                address,
                life_cycle,
            } => {
                debug!(
                    "SetLifecycle with address {} into {:?}",
                    address, life_cycle
                );
                if let Some(actor) = map.get_mut(&address) {
                    actor.4 = life_cycle;
                };
            }
            ActorSystemCmd::RegisterJob { job_id, controller } => {
                debug!("RegisterJob with id {}", job_id);
                let _ = job_controllers.insert(job_id, controller);
            }
            ActorSystemCmd::FindJob { job_id, result_tx } => {
                debug!("FindJob with id {}", job_id);
                let _ = result_tx.send(job_controllers.get(&job_id).cloned());
            }
        };
    }
}
// }}}

// {{{ fn delay_or_abort
/// Sleep for `duration` while staying responsive to the job's control channels.
/// See the doc-comment on the bounded variant for the rationale.
async fn delay_or_abort(
    duration: std::time::Duration,
    abort_rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
    stop_rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
    resume_rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
) -> bool {
    if duration.is_zero() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(duration) => true,
        _ = abort_rx.recv() => false,
        _ = stop_rx.recv() => {
            tokio::select! {
                _ = resume_rx.recv() => true,
                _ = abort_rx.recv() => false,
            }
        }
    }
}
// }}}

// {{{ fn filter_address
fn filter_address(
    address_list: &HashSet<String>,
    regex: &str,
) -> Result<Vec<String>, regex::Error> {
    let regex = regex::Regex::new(&format!("^{}$", regex.replace("*", "(\\S+)"))).map_err(|e| {
        error!("Regex error: {:?}", e);
        e
    })?;
    Ok(address_list
        .iter()
        .filter(|x| regex.is_match(x))
        .map(|x| x.to_string())
        .collect())
}
// }}}
