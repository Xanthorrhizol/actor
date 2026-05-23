use crate::channel;
use crate::{
    ActorError, ActorSystem, ActorSystemCmd, Blocking, CHANNEL_SIZE, ErrorHandling, LifeCycle,
    Message, TypedMailbox,
};
use std::sync::Arc;

/// User-implemented unit of work in the actor system.
///
/// You provide:
/// - associated types `Message`, `Result`, `Error`,
/// - `address()` so the system can place the actor in its address map,
/// - `handle()` to process each inbound message.
///
/// The system provides the rest: a typed mailbox, a tokio task host
/// (per [`Blocking`]), the lifecycle state machine ([`LifeCycle`]), and
/// the optional hooks ([`pre_start`], [`pre_restart`], [`post_stop`],
/// [`post_restart`]) that fire at the relevant transitions.
///
/// Register an actor with `actor.register(&mut system, ...)` (see
/// [`register`]); never implement [`run_actor`] manually — its body is
/// the receive/lifecycle loop the system depends on.
///
/// `'static + Send + Sync` because the actor's state moves into a tokio
/// task that may outlive the caller.
///
/// [`Blocking`]: crate::Blocking
/// [`LifeCycle`]: crate::LifeCycle
/// [`pre_start`]: Self::pre_start
/// [`pre_restart`]: Self::pre_restart
/// [`post_stop`]: Self::post_stop
/// [`post_restart`]: Self::post_restart
/// [`register`]: Self::register
/// [`run_actor`]: Self::run_actor
#[async_trait::async_trait]
pub trait Actor
where
    Self: Sized + Send + Sync + 'static,
{
    /// Inbound message type. `Debug` for log output. With `multi-node` on,
    /// must also implement `xancode::Codec` for cross-node serialization
    /// (enforced via [`MaybeCodec`] on the send methods).
    ///
    /// [`MaybeCodec`]: crate::MaybeCodec
    type Message: std::fmt::Debug + Sized + Send + Sync + 'static;

    /// Reply type returned from `handle`. Visible to callers of
    /// `send_and_recv::<Self>`.
    type Result: std::fmt::Debug + Sized + Send + 'static;

    /// Error type returned from `handle` on failure. Combined with the
    /// per-actor [`ErrorHandling`] policy to decide whether to resume,
    /// restart, or stop the actor.
    ///
    /// [`ErrorHandling`]: crate::ErrorHandling
    type Error: std::fmt::Debug + std::fmt::Display + Send;

    /// Returns this actor's address. Must be stable for the actor's
    /// lifetime — the system uses it as the key in its address map.
    #[cfg(not(feature = "multi-node"))]
    fn address(&self) -> &str;
    /// Returns this actor's fully qualified address (multi-node).
    /// `node` must equal the registering `ActorSystem`'s `node_name`,
    /// otherwise `register` fails with [`AddressNotOwned`].
    ///
    /// [`AddressNotOwned`]: crate::ActorError::AddressNotOwned
    #[cfg(feature = "multi-node")]
    fn address(&self) -> &crate::inter_node::Address;

    /// Internal helper: the local name part of this actor's address.
    /// Single-node: equals `address()`. Multi-node: `address().name`.
    #[doc(hidden)]
    fn local_name(&self) -> &str {
        #[cfg(not(feature = "multi-node"))]
        {
            self.address()
        }
        #[cfg(feature = "multi-node")]
        {
            &self.address().name
        }
    }

    /// Process one inbound message. Called once per message in the actor's
    /// receive loop. The `Arc<Self::Message>` is shared with potential
    /// broadcast recipients — clone the inner payload if you need to
    /// mutate it.
    ///
    /// Return `Err(...)` to trigger this actor's [`ErrorHandling`]
    /// policy: `Resume` drops the message and continues, `Restart` tears
    /// down the mailbox and re-enters via `pre_restart` / `post_restart`,
    /// `Stop` runs `post_stop` and terminates.
    ///
    /// [`ErrorHandling`]: crate::ErrorHandling
    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error>;

    /// Runs once after the actor enters [`LifeCycle::Starting`] but
    /// before transitioning to `Receiving`. Use for one-time setup that
    /// needs `&mut self` (e.g. opening a connection, registering with an
    /// external service).
    ///
    /// [`LifeCycle::Starting`]: crate::LifeCycle::Starting
    async fn pre_start(&mut self) {}

    /// Runs after `post_stop` and before the actor re-enters
    /// `Starting`/`Restarting` for a restart. Use to release / re-prepare
    /// state that needs to be fresh in the next incarnation.
    async fn pre_restart(&mut self) {}

    /// Runs as the actor enters [`LifeCycle::Stopping`] — either due to
    /// `Stop` error handling, a `kill` from `unregister`, or an explicit
    /// `restart`. Last chance to clean up before the task exits.
    ///
    /// [`LifeCycle::Stopping`]: crate::LifeCycle::Stopping
    async fn post_stop(&mut self) {}

    /// Runs at the top of every restart iteration (after the first run),
    /// before `pre_start`'s normal startup work. Symmetric to
    /// `pre_restart`; use for "what to do once the restart actually
    /// begins" rather than "what to do before tearing down".
    async fn post_restart(&mut self) {}

    /// Internal receive/lifecycle loop.
    ///
    /// Owns the mailbox, drains messages, applies `ErrorHandling`,
    /// fires the lifecycle hooks, and reports state transitions back to
    /// `actor_system_loop` via `ActorSystemCmd`. Do **not** implement
    /// this method — use [`register`] to wire the actor up.
    ///
    /// `channel_size` is the mailbox capacity under `bounded-channel`
    /// and is ignored under `unbounded-channel`.
    ///
    /// [`register`]: Self::register
    async fn run_actor(
        &mut self,
        actor_system_tx: channel::Sender<ActorSystemCmd>,
        error_handling: ErrorHandling,
        ready_tx: channel::Sender<Result<(), ActorError>>,
        channel_size: Option<usize>,
    ) -> Result<(), ActorError> {
        let mut is_restarted = false;
        let size = channel_size.unwrap_or(CHANNEL_SIZE);
        loop {
            if is_restarted {
                self.post_restart().await;
            }
            let (tx, mut rx) = channel::channel::<Message<Self>>(size);
            let (kill_tx, mut kill_rx) = channel::channel::<()>(size);
            let (restart_tx, mut restart_rx) = channel::channel::<()>(size);
            let mailbox = Arc::new(TypedMailbox::<Self>::new(tx.clone()));

            let mut count = 0;
            let result_rx = loop {
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = channel::send(
                    &actor_system_tx,
                    ActorSystemCmd::Register {
                        actor_type: std::any::type_name::<Self>().to_string(),
                        #[cfg(not(feature = "multi-node"))]
                        address: self.address().to_string(),
                        #[cfg(feature = "multi-node")]
                        address: self.address().clone(),
                        mailbox: mailbox.clone(),
                        restart_tx: restart_tx.clone(),
                        kill_tx: kill_tx.clone(),
                        life_cycle: if is_restarted {
                            LifeCycle::Restarting
                        } else {
                            LifeCycle::Starting
                        },
                        result_tx,
                        is_restarted,
                    },
                )
                .await
                {
                    count += 1;
                    error!(
                        "Failed to register actor {}...({}): {:?}",
                        self.local_name(),
                        count,
                        e
                    );
                    if count > 10 {
                        let _ = channel::send(&ready_tx, Err(ActorError::UnhealthyActorSystem))
                            .await;
                        return Err(ActorError::UnhealthyActorSystem);
                    }
                }
                break result_rx;
            };
            match result_rx.await {
                Ok(Err(e)) => {
                    let _ = channel::send(&ready_tx, Err(e)).await;
                    // Now, this case is only when the address already exists.
                    return Err(ActorError::AddressAlreadyExist(self.local_name().to_string()));
                }
                Err(e) => {
                    let _ = channel::send(&ready_tx, Err(ActorError::from(e))).await;
                    return Err(ActorError::UnhealthyActorSystem);
                }
                _ => {}
            }
            self.pre_start().await;
            is_restarted = true;
            let _ = channel::send(
                &actor_system_tx,
                ActorSystemCmd::SetLifeCycle {
                    address: self.local_name().to_string(),
                    life_cycle: LifeCycle::Receiving,
                },
            )
            .await;
            let _ = channel::send(&ready_tx, Ok(())).await;
            if let Some(_) = loop {
                tokio::select! {
                    Some(mut msg) = rx.recv() => {
                        let result_tx = msg.result_tx();
                        let msg_de = msg.inner();
                        match self.handle(msg_de).await {
                           Ok(result) => {
                                if let Some(result_tx) = result_tx {
                                    let _ = result_tx.send(result);
                                }
                            }
                           Err(e) => {
                                match error_handling {
                                    ErrorHandling::Resume => {
                                        debug!("Handler's result has error: {:?} ...Resume this actor", e);
                                        continue;
                                    }
                                    ErrorHandling::Restart => {
                                        debug!("Handler's result has error: {:?} ...Restart this actor", e);
                                        break None;
                                    }
                                    ErrorHandling::Stop => {
                                        error!("Handler's result has error: {:?} ...Stop this actor", e);
                                        break Some(());
                                    }
                                }
                           }
                       }
                    }
                    Some(_) = kill_rx.recv() => {
                        info!("Kill actor: address={}", self.local_name());
                        break Some(());
                    }
                    Some(_) = restart_rx.recv() => {
                        info!("Restart actor: address={}", self.local_name());
                        break None;
                    }
                };
            } {
                let _ = channel::send(
                    &actor_system_tx,
                    ActorSystemCmd::SetLifeCycle {
                        address: self.local_name().to_string(),
                        life_cycle: LifeCycle::Stopping,
                    },
                )
                .await;
                self.post_stop().await;
                let _ = channel::send(
                    &actor_system_tx,
                    ActorSystemCmd::SetLifeCycle {
                        address: self.local_name().to_string(),
                        life_cycle: LifeCycle::Terminated,
                    },
                )
                .await;
                break Ok(());
            }
            let _ = channel::send(
                &actor_system_tx,
                ActorSystemCmd::SetLifeCycle {
                    address: self.local_name().to_string(),
                    life_cycle: LifeCycle::Stopping,
                },
            )
            .await;
            self.pre_restart().await;
            let _ = channel::send(
                &actor_system_tx,
                ActorSystemCmd::SetLifeCycle {
                    address: self.local_name().to_string(),
                    life_cycle: LifeCycle::Restarting,
                },
            )
            .await;
        }
    }

    /// Register the actor with `actor_system` and spawn its receive loop.
    ///
    /// Consumes `self` (the actor's state moves into the spawned task).
    /// Awaits until the system confirms registration (or rejects it with
    /// `AddressAlreadyExist` / `AddressNotOwned`), so on `Ok(())` the
    /// actor is wired up — though it may still be in [`LifeCycle::Starting`]
    /// when this returns.
    ///
    /// - `error_handling`: per-actor [`ErrorHandling`] policy applied
    ///   on every `handle` error.
    /// - `blocking`: chooses between `tokio::spawn` (`NonBlocking`) and
    ///   `spawn_blocking + block_on` (`Blocking`). Pick `Blocking` if the
    ///   handler does long synchronous work that would otherwise starve
    ///   the async runtime.
    /// - `channel_size`: override the mailbox capacity for this actor.
    ///   `None` uses the default (4096). Ignored under
    ///   `unbounded-channel`.
    ///
    /// [`ErrorHandling`]: crate::ErrorHandling
    /// [`LifeCycle::Starting`]: crate::LifeCycle::Starting
    async fn register(
        mut self,
        actor_system: &mut ActorSystem,
        error_handling: ErrorHandling,
        blocking: Blocking,
        channel_size: Option<usize>,
    ) -> Result<(), ActorError> {
        let size = channel_size.unwrap_or(CHANNEL_SIZE);
        let (tx, mut rx) = channel::channel(size);
        let actor_system_tx = actor_system.handler_tx();
        let _ = if blocking == Blocking::Blocking {
            tokio::task::spawn_blocking(move || {
                let result = tokio::runtime::Handle::current().block_on(self.run_actor(
                    actor_system_tx,
                    error_handling,
                    tx,
                    channel_size,
                ));
                if let Err(e) = result {
                    error!("Actor {} run failed: {:?}", self.local_name(), e);
                }
            })
        } else {
            tokio::spawn(async move {
                let result = self
                    .run_actor(actor_system_tx, error_handling, tx, channel_size)
                    .await;
                if let Err(e) = result {
                    error!("Actor {} run failed: {:?}", self.local_name(), e);
                }
            })
        };
        if let Some(result) = rx.recv().await {
            result
        } else {
            Err(ActorError::ChannelRecv)
        }
    }
}
