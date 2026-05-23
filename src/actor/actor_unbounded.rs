use crate::{
    ActorError, ActorSystem, ActorSystemCmd, Blocking, ErrorHandling, LifeCycle, Message,
    TypedMailbox,
};
use std::sync::Arc;

/// User-implemented unit of work in the actor system. (unbounded-channel variant.)
///
/// Same semantics as the bounded variant — see the bounded `Actor` doc
/// for the trait-level overview. The only differences are
/// `Actor::register` not taking a `channel_size` parameter and the
/// underlying mailbox being `tokio::sync::mpsc::unbounded_channel`
/// instead of `channel(capacity)`.
#[async_trait::async_trait]
pub trait Actor
where
    Self: Sized + Send + Sync + 'static,
{
    /// Inbound message type. `Debug` for log output. With `multi-node` on,
    /// must also implement `xancode::Codec`.
    type Message: std::fmt::Debug + Sized + Send + Sync + 'static;

    /// Reply type returned from `handle`. Visible to `send_and_recv::<Self>` callers.
    type Result: std::fmt::Debug + Sized + Send + 'static;

    /// Error type returned from `handle`. Combined with `ErrorHandling`
    /// to decide resume / restart / stop.
    type Error: std::fmt::Debug + std::fmt::Display + Send;

    /// Returns this actor's address. Must be stable for the actor's lifetime.
    #[cfg(not(feature = "multi-node"))]
    fn address(&self) -> &str;
    /// Returns this actor's fully qualified address (multi-node).
    /// `node` must equal the registering `ActorSystem`'s `node_name`.
    #[cfg(feature = "multi-node")]
    fn address(&self) -> &crate::inter_node::Address;

    /// Internal helper: the local name part of this actor's address.
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

    /// Process one inbound message. Return `Err` to trigger this actor's
    /// `ErrorHandling` policy.
    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error>;

    /// One-time setup before entering `Receiving`.
    async fn pre_start(&mut self) {}

    /// Runs before the actor restarts (after `post_stop`, before the next `pre_start`).
    async fn pre_restart(&mut self) {}

    /// Runs as the actor enters `Stopping` (also before a restart).
    async fn post_stop(&mut self) {}

    /// Runs at the top of every restart iteration after the first run.
    async fn post_restart(&mut self) {}

    /// Internal receive/lifecycle loop. Do not implement manually —
    /// use [`register`] to wire the actor up.
    ///
    /// [`register`]: Self::register
    async fn run_actor(
        &mut self,
        actor_system_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd>,
        error_handling: ErrorHandling,
        ready_tx: tokio::sync::mpsc::UnboundedSender<Result<(), ActorError>>,
    ) -> Result<(), ActorError> {
        let mut is_restarted = false;
        loop {
            if is_restarted {
                self.post_restart().await;
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message<Self>>();
            let (kill_tx, mut kill_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let (restart_tx, mut restart_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let mailbox = Arc::new(TypedMailbox::<Self>::new(tx.clone()));

            let mut count = 0;
            let result_rx = loop {
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = actor_system_tx.send(ActorSystemCmd::Register {
                    actor_type: std::any::type_name::<Self>().to_string(),
                    #[cfg(not(feature = "multi-node"))]
                    address: self.local_name().to_string(),
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
                }) {
                    count += 1;
                    error!(
                        "Failed to register actor {}...({}): {:?}",
                        self.local_name(),
                        count,
                        e
                    );
                    if count > 10 {
                        let _ = ready_tx.send(Err(ActorError::UnhealthyActorSystem));
                        return Err(ActorError::UnhealthyActorSystem);
                    }
                }
                break result_rx;
            };
            match result_rx.await {
                Ok(Err(e)) => {
                    let _ = ready_tx.send(Err(e));
                    // Now, this case is only when the address already exists.
                    return Err(ActorError::AddressAlreadyExist(self.local_name().to_string()));
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(ActorError::from(e)));
                    return Err(ActorError::UnhealthyActorSystem);
                }
                _ => {}
            }
            self.pre_start().await;
            is_restarted = true;
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle {
                address: self.local_name().to_string(),
                life_cycle: LifeCycle::Receiving,
            });
            let _ = ready_tx.send(Ok(()));
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
                let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle {
                    address: self.local_name().to_string(),
                    life_cycle: LifeCycle::Stopping,
                });
                self.post_stop().await;
                let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle {
                    address: self.local_name().to_string(),
                    life_cycle: LifeCycle::Terminated,
                });
                break Ok(());
            }
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle {
                address: self.local_name().to_string(),
                life_cycle: LifeCycle::Stopping,
            });
            self.pre_restart().await;
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle {
                address: self.local_name().to_string(),
                life_cycle: LifeCycle::Restarting,
            });
        }
    }

    /// Register the actor with `actor_system` and spawn its receive loop.
    ///
    /// Consumes `self`. Awaits until the system confirms (or rejects)
    /// registration. `error_handling` sets the per-actor policy applied
    /// on every `handle` error; `blocking` chooses between `tokio::spawn`
    /// and `spawn_blocking + block_on` for the actor's host task.
    ///
    /// Unlike the bounded variant, there is no `channel_size` parameter
    /// — the mailbox is an unbounded mpsc.
    async fn register(
        mut self,
        actor_system: &mut ActorSystem,
        error_handling: ErrorHandling,
        blocking: Blocking,
    ) -> Result<(), ActorError> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let actor_system_tx = actor_system.handler_tx();
        let _ = if blocking == Blocking::Blocking {
            tokio::task::spawn_blocking(move || {
                let result = tokio::runtime::Handle::current().block_on(self.run_actor(
                    actor_system_tx,
                    error_handling,
                    tx,
                ));
                if let Err(e) = result {
                    error!("Actor {} run failed: {:?}", self.local_name(), e);
                }
            })
        } else {
            tokio::spawn(async move {
                let result = self.run_actor(actor_system_tx, error_handling, tx).await;
                if let Err(e) = result {
                    error!("Actor {} run failed: {:?}", self.local_name(), e);
                }
            })
        };
        if let Some(result) = rx.recv().await {
            result
        } else {
            Err(ActorError::UnboundedChannelRecv)
        }
    }
}
