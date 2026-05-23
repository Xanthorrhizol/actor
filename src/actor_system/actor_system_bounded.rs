use crate::{
    Actor, ActorError, CHANNEL_SIZE, JobController, JobSpec, LifeCycle, Mailbox, MaybeCodec,
    RunJobResult,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(feature = "multi-node")]
use crate::inter_node::InterNodeRuntime;

/// Wire protocol between `ActorSystem` and the single `actor_system_loop`
/// task that owns the registry. Most users never construct these directly
/// — the public `ActorSystem` methods (`send`, `register`, `restart`,
/// `run_job`, etc.) wrap each variant.
///
/// Exposed (and reachable via [`ActorSystem::handler_tx`]) so advanced
/// callers can drive the loop manually — e.g. to issue several
/// `FindActor` lookups without going through the cache.
pub enum ActorSystemCmd {
    /// Insert a fresh actor into the registry. `is_restarted = true`
    /// permits replacing an existing entry (used by the restart cycle);
    /// `false` rejects duplicates with `AddressAlreadyExist`.
    Register {
        actor_type: String,
        #[cfg(not(feature = "multi-node"))]
        address: String,
        #[cfg(feature = "multi-node")]
        address: crate::inter_node::Address,
        mailbox: Arc<dyn Mailbox>,
        restart_tx: tokio::sync::mpsc::Sender<()>,
        kill_tx: tokio::sync::mpsc::Sender<()>,
        life_cycle: LifeCycle,
        result_tx: tokio::sync::oneshot::Sender<Result<(), ActorError>>,
        is_restarted: bool,
    },
    /// Trigger the restart cycle for every actor whose name matches the
    /// regex. Backs `ActorSystem::restart`.
    Restart {
        address_regex: String,
    },
    /// Tear down every actor whose name matches the regex. Backs
    /// `ActorSystem::unregister`.
    Unregister {
        address_regex: String,
    },
    /// Snapshot of address names matching the regex. Backs
    /// `filter_address` and the local side of `send_broadcast`.
    FilterAddress {
        address_regex: String,
        result_tx: tokio::sync::oneshot::Sender<Vec<String>>,
    },
    /// Look up a specific actor by `(actor_type, address)`. Returns the
    /// mailbox plus a `ready` flag (false while the actor is in
    /// `Starting`/`Restarting`). Backs the send-family methods'
    /// post-cache lookups.
    FindActor {
        actor_type: String,
        address: String,
        result_tx: tokio::sync::oneshot::Sender<
            Option<(Arc<dyn Mailbox>, bool)>, // mailbox, ready
        >,
    },
    /// Update an actor's `LifeCycle`. Pushed by the actor's own
    /// `run_actor` loop at each transition.
    SetLifeCycle {
        address: String,
        life_cycle: LifeCycle,
    },
    /// Register a job's `JobController` under `job_id`. Pushed by
    /// `run_job` after spawning the job loop, so `abort_job` / `stop_job`
    /// / `resume_job` can later look it up.
    RegisterJob {
        job_id: String,
        controller: JobController,
    },
    /// Look up a `JobController` by id. Backs `abort_job` / `stop_job` /
    /// `resume_job`.
    FindJob {
        job_id: String,
        result_tx: tokio::sync::oneshot::Sender<Option<JobController>>,
    },
}

#[derive(Clone)]
/// Main entry point for spawning, addressing, and messaging actors.
///
/// Owns one background task — the `actor_system_loop` — that holds the
/// authoritative registry of `(actor_type, address) → mailbox` mappings.
/// All public methods funnel through that loop via [`Self::handler_tx`],
/// which keeps registry mutations strictly sequential.
///
/// `Clone` is cheap: clones share the same handler channel and (under
/// `multi-node`) the same `InterNodeRuntime`. Each clone has its own
/// `cache` (a local `HashMap` of mailboxes for fast resends without
/// touching the loop), so caching is per-clone, not global.
///
/// Under the `multi-node` feature, an `ActorSystem` may also carry a
/// xanq broker connection; `send`/`send_and_recv`/`run_job` automatically
/// route to the right node based on `Address::node`.
pub struct ActorSystem {
    handler_tx: tokio::sync::mpsc::Sender<ActorSystemCmd>,
    cache: HashMap<String, (String, Arc<dyn Mailbox>)>,
    channel_size: usize,
    /// The cluster identity of this system. Always set when `multi-node` is on.
    #[cfg(feature = "multi-node")]
    node_name: String,
    /// Some when a broker connection was provided; None means local-only
    /// (sends to other nodes will fail with `InterNodeNotConfigured`).
    #[cfg(feature = "multi-node")]
    inter_node: Option<InterNodeRuntime>,
}

#[cfg(not(feature = "multi-node"))]
impl Default for ActorSystem {
    fn default() -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::channel(CHANNEL_SIZE);
        let mut me = Self {
            handler_tx,
            cache: HashMap::new(),
            channel_size: CHANNEL_SIZE,
        };
        me.run(handler_rx);
        me
    }
}

impl ActorSystem {
    /// Spin up a new `ActorSystem` and its backing loop task.
    ///
    /// `channel_size` overrides the default `CHANNEL_SIZE` (4096) for
    /// the command channel that feeds the loop; `None` keeps the
    /// default. Returned eagerly — the loop task is already running by
    /// the time this returns.
    #[cfg(not(feature = "multi-node"))]
    pub fn new(channel_size: Option<usize>) -> Self {
        let (handler_tx, handler_rx) =
            tokio::sync::mpsc::channel(channel_size.unwrap_or(CHANNEL_SIZE));
        let mut me = Self {
            handler_tx,
            cache: HashMap::new(),
            channel_size: channel_size.unwrap_or(CHANNEL_SIZE),
        };
        me.run(handler_rx);
        me
    }

    /// Spin up a new `ActorSystem` under `multi-node`.
    ///
    /// - `channel_size`: as in the single-node variant.
    /// - `node_name`: this system's identity in the cluster; embedded in
    ///   every [`Address`] it owns and used as the xanq topic prefix.
    /// - `broker_addr`: pass `Some(addr)` to connect to a xanq broker and
    ///   participate in inter-node delivery; pass `None` to run local-only
    ///   (cross-node `send`/`send_and_recv`/`run_job` will fail with
    ///   [`ActorError::InterNodeNotConfigured`]).
    ///
    /// When a broker is configured the connect uses
    /// [`DEFAULT_BROKER_CONNECT_TIMEOUT`] so a missing broker fails
    /// promptly. After the connect succeeds the request and response
    /// consumer tasks are spawned automatically — no further setup is
    /// required.
    ///
    /// [`Address`]: crate::inter_node::Address
    /// [`DEFAULT_BROKER_CONNECT_TIMEOUT`]: crate::inter_node::DEFAULT_BROKER_CONNECT_TIMEOUT
    #[cfg(feature = "multi-node")]
    pub async fn new(
        channel_size: Option<usize>,
        node_name: String,
        broker_addr: Option<String>,
    ) -> Result<Self, ActorError> {
        let (handler_tx, handler_rx) =
            tokio::sync::mpsc::channel(channel_size.unwrap_or(CHANNEL_SIZE));
        let inter_node = match broker_addr {
            Some(addr) => Some(InterNodeRuntime::connect(node_name.clone(), addr).await?),
            None => None,
        };
        let mut me = Self {
            handler_tx,
            cache: HashMap::new(),
            channel_size: channel_size.unwrap_or(CHANNEL_SIZE),
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

    /// Forward an already-decoded payload to a local actor identified by
    /// `(actor_type, name)`. Returns once the mailbox accepts the message;
    /// does not wait for the handler to run.
    ///
    /// Public because the inter-node request consumer
    /// ([`InterNodeRuntime::start_consumers`]) calls it after decoding an
    /// incoming `InterNodeMessage::Fire` envelope. Application code should
    /// prefer [`Self::send`].
    ///
    /// [`InterNodeRuntime::start_consumers`]: crate::inter_node::InterNodeRuntime::start_consumers
    #[cfg(feature = "multi-node")]
    pub async fn dispatch_local_any(
        &self,
        actor_type: String,
        address: String,
        payload: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), ActorError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor {
                actor_type,
                address: address.clone(),
                result_tx: tx,
            })
            .await;
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

    /// Request-response counterpart to [`Self::dispatch_local_any`].
    /// Awaits the handler's return value as `Box<dyn Any>`, which the
    /// caller is expected to encode (typically via
    /// [`encode_result_for`]) before shipping back in an
    /// [`InterNodeResponse`].
    ///
    /// [`encode_result_for`]: crate::inter_node::encode_result_for
    /// [`InterNodeResponse`]: crate::inter_node::InterNodeResponse
    #[cfg(feature = "multi-node")]
    pub async fn dispatch_local_any_and_recv(
        &self,
        actor_type: String,
        address: String,
        payload: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<Box<dyn std::any::Any + Send>, ActorError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor {
                actor_type,
                address: address.clone(),
                result_tx: tx,
            })
            .await;
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
        let channel_size = self.channel_size;

        let (abort_tx, mut abort_rx) = tokio::sync::mpsc::channel(channel_size);
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel(channel_size);
        let (resume_tx, mut resume_rx) = tokio::sync::mpsc::channel(channel_size);

        let result_subscriber_rx = if subscribe {
            let (sub_tx, sub_rx) = tokio::sync::mpsc::channel(channel_size);
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
                    let _ = sub_tx.send(outcome).await;
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

        let _ = self
            .handler_tx
            .send(ActorSystemCmd::RegisterJob {
                job_id: job_id.clone(),
                controller: JobController {
                    abort_tx,
                    stop_tx,
                    resume_tx,
                },
            })
            .await;

        Ok(RunJobResult {
            job_id,
            result_subscriber_rx,
        })
    }

    /// Clone of the loop's command channel.
    ///
    /// Lets advanced callers drive the registry directly with
    /// [`ActorSystemCmd`] variants — typically to bypass the per-clone TX
    /// cache or to issue several `FindActor`/`FilterAddress` lookups
    /// without going through the helper methods. The channel is the same
    /// one all built-in methods use, so commands are interleaved in
    /// arrival order with regular traffic.
    pub fn handler_tx(&self) -> tokio::sync::mpsc::Sender<ActorSystemCmd> {
        self.handler_tx.clone()
    }

    /// Snapshot of local actor addresses whose names match `address_regex`.
    ///
    /// Uses `*` as a wildcard (converted to the `(\S+)` regex
    /// alternative); the pattern is anchored as `^...$`. Returns an empty
    /// vector if the loop drops the response — never panics.
    pub async fn filter_address(&self, address_regex: String) -> Vec<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FilterAddress {
                address_regex,
                result_tx: tx,
            })
            .await;
        match rx.await {
            Ok(addresses) => addresses,
            Err(e) => {
                error!("Receive address list failed: {:?}", e);
                Vec::new()
            }
        }
    }

    /// Restart every local actor whose name matches `address_regex`
    /// (`*` wildcard syntax, same as [`filter_address`]).
    ///
    /// Fire-and-forget — sends a `Restart` command and returns. Each
    /// matched actor goes `Receiving → Stopping → Restarting → Starting →
    /// Receiving` via the lifecycle hooks; this method does not wait for
    /// the cycle to complete, so a `send` immediately after `restart` may
    /// hit an actor still in `Restarting` and get [`ActorError::ActorNotReady`]
    /// after the system's retry budget.
    ///
    /// [`filter_address`]: Self::filter_address
    pub fn restart(&mut self, address_regex: String) {
        if let Err(e) = self
            .handler_tx
            .try_send(ActorSystemCmd::Restart { address_regex })
        {
            error!("Send restart command failed: {:?}", e);
        }
    }

    /// Unregister every local actor whose name matches `address_regex`
    /// and tear down its task. Fire-and-forget.
    ///
    /// Each matched actor receives a kill signal, runs `post_stop`,
    /// transitions to `Terminated`, and is removed from the local map.
    /// `"*"` matches everything — convenient for "kill them all" at
    /// shutdown.
    pub fn unregister(&mut self, address_regex: String) {
        if let Err(e) = self
            .handler_tx
            .try_send(ActorSystemCmd::Unregister { address_regex })
        {
            error!("Send unregister command failed: {:?}", e);
        }
    }

    /// Fire-and-forget send to a single actor.
    ///
    /// Tries the per-clone TX cache first; on miss, asks the loop for the
    /// mailbox via `FindActor` and (on success) caches it for next time.
    /// If the actor is registered but not yet `Receiving`, retries up to
    /// 10 times with 100 ms sleeps before returning
    /// [`ActorError::ActorNotReady`].
    ///
    /// Under `multi-node`, if `address.node != self.node_name` the
    /// message is encoded via `xancode::Codec` and shipped through the
    /// broker as `InterNodeMessage::Fire` (no retry — broker delivery is
    /// best-effort once accepted).
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
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

    /// Cache-bypassing variant of [`send`].
    ///
    /// Always issues a fresh `FindActor` against the loop — useful when
    /// you don't want to retain a mailbox `Arc` (e.g. one-shot sends) or
    /// when you're calling from `&self` and the cache mutation isn't
    /// allowed. Same retry policy and the same multi-node routing as
    /// [`send`].
    ///
    /// [`send`]: Self::send
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
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
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FilterAddress {
                address_regex,
                result_tx: tx,
            })
            .await;
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
                let _ = self
                    .handler_tx
                    .send(ActorSystemCmd::FindActor {
                        actor_type: actor_type.to_string(),
                        address: address.clone(),
                        result_tx: tx,
                    })
                    .await;
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

    /// Sends a message across the nodes selected by `filter`.
    ///
    /// `address_regex` is the `*`-wildcard pattern used by [`filter_address`];
    /// on each targeted node it's matched against that node's local actor
    /// addresses.
    ///
    /// Returns a [`crate::inter_node::BroadcastResult`] with two separately
    /// counted vectors:
    /// - `local` — per-actor results for the caller's own local fan-out
    ///   (matched actors on this node).
    /// - `remote` — per-peer envelope acks for `BroadcastFire` sends. Each
    ///   peer regex-matches against *its own* local actors and dispatches
    ///   fire-and-forget, so `remote.len()` does **not** tell you how many
    ///   remote actors actually received the message.
    ///
    /// `NodeFilter::Peers` containing only the caller's own node degenerates
    /// to a `SelfOnly` fan-out (no broker traffic).
    ///
    /// [`filter_address`]: crate::ActorSystem::filter_address
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

    /// Local regex fan-out helper (no cache). One entry per matched local actor.
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
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FilterAddress {
                address_regex,
                result_tx: tx,
            })
            .await;
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
                let _ = self
                    .handler_tx
                    .send(ActorSystemCmd::FindActor {
                        actor_type: actor_type.to_string(),
                        address: address.clone(),
                        result_tx: tx,
                    })
                    .await;
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

    /// Cache-bypassing variant of [`send_broadcast`] for multi-node.
    /// Same `BroadcastResult { local, remote }` shape and the same per-actor
    /// vs. per-peer count semantics — see `send_broadcast`'s docs for the
    /// details. Skips the local TX cache, so every local match re-issues
    /// `FindActor`.
    ///
    /// [`send_broadcast`]: Self::send_broadcast
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

    /// Request-response send: deliver `msg` and wait for the handler's
    /// `T::Result`.
    ///
    /// Uses the TX cache and the same readiness retry policy as
    /// [`send`]. On a cross-node address the call goes out as
    /// `InterNodeMessage::Call`; the response is matched back via the
    /// `req_id` in the pending-requests map and decoded via
    /// `xancode::Codec`.
    ///
    /// Errors:
    /// - [`ActorError::AddressNotFound`] / [`ActorError::ActorNotReady`]
    ///   when the local lookup fails or the actor never becomes ready.
    /// - [`ActorError::MessageTypeMismatch`] if the handler returns a
    ///   different concrete type than `T::Result` (only possible if you
    ///   bypass the trait at compile time).
    /// - `ActorError::InterNodeRemote` / `ActorError::InterNodeDecode`
    ///   for cross-node failures.
    ///
    /// [`send`]: Self::send
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
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

    /// Cache-bypassing variant of [`send_and_recv`]. Same semantics,
    /// always issues a fresh `FindActor`.
    ///
    /// [`send_and_recv`]: Self::send_and_recv
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
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

    /// Spawn a background job that delivers `msg` to `address` on the
    /// schedule described by [`JobSpec`].
    ///
    /// - `subscribe = true` sends `send_and_recv` per iteration and pipes
    ///   each result through `RunJobResult::result_subscriber_rx`;
    ///   `false` uses fire-and-forget `send` and returns `None` for the
    ///   subscriber.
    /// - `job_id`: provide one to address the job later with
    ///   [`abort_job`]/[`stop_job`]/[`resume_job`]; `None` generates a
    ///   fresh UUID.
    ///
    /// The job loop uses `tokio::select!` to race the sleep against the
    /// abort/stop/resume channels, so abort/stop are responsive even
    /// mid-sleep. Returns once the actor lookup succeeds and the loop
    /// has been spawned; the loop itself runs until `max_iter` is
    /// reached, the job is aborted, or — for `interval = None` — the
    /// single iteration completes.
    ///
    /// Under `multi-node`, a cross-node address spawns an analogous loop
    /// that issues `InterNodeMessage::Call`/`Fire` envelopes via the
    /// broker on the same schedule.
    ///
    /// [`abort_job`]: Self::abort_job
    /// [`stop_job`]: Self::stop_job
    /// [`resume_job`]: Self::resume_job
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
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
            let (sub_tx, sub_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (abort_tx, mut abort_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (resume_tx, mut resume_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let payload = payload.clone();
            let _ = tokio::spawn(async move {
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
                    let result = match mailbox.send_and_recv(payload.clone()).await {
                        Ok(result_any) => result_any
                            .downcast::<T::Result>()
                            .map(|x| Ok(*x))
                            .unwrap_or_else(|_| Err(ActorError::MessageTypeMismatch)),
                        Err(e) => Err(e),
                    };
                    let _ = sub_tx.send(result).await;
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::RegisterJob {
                    job_id: job_id.clone(),
                    controller: JobController {
                        abort_tx,
                        stop_tx,
                        resume_tx,
                    },
                })
                .await;
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: Some(sub_rx),
            });
        } else {
            let (abort_tx, mut abort_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (resume_tx, mut resume_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let _ = tokio::spawn(async move {
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::RegisterJob {
                    job_id: job_id.clone(),
                    controller: JobController {
                        abort_tx,
                        stop_tx,
                        resume_tx,
                    },
                })
                .await;
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: None,
            });
        }
    }

    /// Cache-bypassing variant of [`run_job`]. Same semantics; the
    /// resolved mailbox is not inserted into the TX cache, so subsequent
    /// `send` calls to the same address will issue another `FindActor`.
    ///
    /// [`run_job`]: Self::run_job
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::FindActor {
                    actor_type: actor_type.to_string(),
                    address: address.clone(),
                    result_tx: tx,
                })
                .await;
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
            let (sub_tx, sub_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let payload = payload.clone();
            let (abort_tx, mut abort_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (resume_tx, mut resume_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let _ = tokio::spawn(async move {
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
                    let result = match mailbox.send_and_recv(payload.clone()).await {
                        Ok(result_any) => result_any
                            .downcast::<T::Result>()
                            .map(|x| Ok(*x))
                            .unwrap_or_else(|_| Err(ActorError::MessageTypeMismatch)),
                        Err(e) => Err(e),
                    };
                    let _ = sub_tx.send(result).await;
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::RegisterJob {
                    job_id: job_id.clone(),
                    controller: JobController {
                        abort_tx,
                        stop_tx,
                        resume_tx,
                    },
                })
                .await;
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: Some(sub_rx),
            });
        } else {
            let (abort_tx, mut abort_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let (resume_tx, mut resume_rx) = tokio::sync::mpsc::channel(self.channel_size);
            let _ = tokio::spawn(async move {
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
            let _ = self
                .handler_tx
                .send(ActorSystemCmd::RegisterJob {
                    job_id: job_id.clone(),
                    controller: JobController {
                        abort_tx,
                        stop_tx,
                        resume_tx,
                    },
                })
                .await;
            return Ok(RunJobResult {
                job_id,
                result_subscriber_rx: None,
            });
        }
    }

    /// Abort a running job: signal its loop to exit at the next wait
    /// point (`start_at` wait, inter-iteration sleep, or during a pause).
    /// The job is removed; subsequent `stop_job` / `resume_job` for the
    /// same id are no-ops.
    ///
    /// Honored within milliseconds — abort is raced against the loop's
    /// sleep via `tokio::select!`, so you don't wait out the remaining
    /// `interval`.
    pub async fn abort_job(&self, job_id: String) {
        info!("Aborting job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindJob {
                job_id: job_id.clone(),
                result_tx: tx,
            })
            .await;
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

    /// Pause a running job: the loop finishes the current iteration (if
    /// in flight) and then waits on `resume_tx`. Pair with [`resume_job`]
    /// to continue, or with [`abort_job`] to terminate while paused.
    ///
    /// [`resume_job`]: Self::resume_job
    /// [`abort_job`]: Self::abort_job
    pub async fn stop_job(&self, job_id: String) {
        info!("Stopping job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindJob {
                job_id: job_id.clone(),
                result_tx: tx,
            })
            .await;
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

    /// Resume a [`stop_job`]'d job. The loop wakes up and immediately
    /// starts the next iteration's `start_at` wait (which is usually
    /// zero by the time you resume).
    ///
    /// [`stop_job`]: Self::stop_job
    pub async fn resume_job(&self, job_id: String) {
        info!("Resuming job {}", job_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindJob {
                job_id: job_id.clone(),
                result_tx: tx,
            })
            .await;
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
        handler_rx: tokio::sync::mpsc::Receiver<ActorSystemCmd>,
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
    mut handler_rx: tokio::sync::mpsc::Receiver<ActorSystemCmd>,
    #[cfg(feature = "multi-node")] inter_node: Option<InterNodeRuntime>,
) {
    let mut address_list = HashSet::<String>::new();
    let mut actor_map = HashMap::<
        String,
        (
            String,
            Arc<dyn Mailbox>,
            tokio::sync::mpsc::Sender<()>,
            tokio::sync::mpsc::Sender<()>,
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
                if actor_map.contains_key(&name) && !is_restarted {
                    let _ = result_tx.send(Err(ActorError::AddressAlreadyExist(name)));
                    continue;
                }
                actor_map.insert(
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
                        actor_map.get_mut(&address)
                    {
                        *life_cycle = LifeCycle::Restarting;
                        let _ = restart_tx.send(()).await;
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
                    match actor_map.entry(address.to_string()) {
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            let _ = entry.get_mut().3.send(()).await;
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
                    actor_map.get(&address)
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
                if let Some(actor) = actor_map.get_mut(&address) {
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
///
/// Returns:
/// - `true` if the delay elapsed normally (or was zero), or if the job was
///   paused via `stop_tx` and later resumed via `resume_tx`.
/// - `false` if `abort_tx` fired at any point (during the delay, or during a
///   pause). Callers should exit immediately on `false`.
///
/// Replaces the older `try_recv` polling + `sleep(interval).await` shape,
/// which had two problems: (1) abort/stop signals were only checked at the
/// top of each iteration, so a fired abort waited up to `interval` to take
/// effect; (2) `resume_rx.recv()` was awaited unconditionally during a stop,
/// so abort couldn't break a paused job.
async fn delay_or_abort(
    duration: std::time::Duration,
    abort_rx: &mut tokio::sync::mpsc::Receiver<()>,
    stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
    resume_rx: &mut tokio::sync::mpsc::Receiver<()>,
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
