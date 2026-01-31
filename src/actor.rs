use crate::{
    ActorError, ActorSystem, ActorSystemCmd, Blocking, ErrorHandling, LifeCycle, Message,
    TypedMailbox,
};
use std::sync::Arc;

#[async_trait::async_trait]
/// Trait for actors in the actor system
/// An actor is a fundamental unit of computation that encapsulates state and behavior.
/// Actors communicate with each other by sending messages, and they can be created, restarted, or stopped by the actor system.
/// Actors can handle messages asynchronously and can be in different states (e.g., starting, receiving, stopping, restarting, terminated).
/// Actors can also implement pre-start, pre-restart, post-stop, and post-restart hooks to perform actions at different lifecycle stages.
/// Actors must implement the `actor` method to handle incoming messages and return results.
/// Actors must also implement the `address` method to provide a unique identifier for the actor.
/// Actors can be registered with the actor system using the `register` method, which will start the actor and handle its lifecycle.
pub trait Actor
where
    Self: Sized + Send + Sync + 'static,
{
    /// The message type that the actor can handle.
    type Message: std::fmt::Debug + Sized + Send + Sync + 'static;

    /// The result type that the actor returns after processing a message.
    type Result: std::fmt::Debug + Sized + Send + 'static;

    /// The error type that the actor can return.
    type Error: std::fmt::Debug + std::fmt::Display + Send;

    /// The address of the actor, which is a unique identifier for the actor in the actor system.
    fn address(&self) -> &str;

    /// Handles incoming messages sent to the actor.
    async fn actor(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error>;

    /// Pre-start hook that is called before the actor starts processing messages.
    async fn pre_start(&mut self) {}

    /// Pre-restart hook that is called before the actor restarts.
    async fn pre_restart(&mut self) {}

    /// Post-stop hook that is called after the actor stops processing messages.
    async fn post_stop(&mut self) {}

    /// Post-restart hook that is called after the actor restarts.
    async fn post_restart(&mut self) {}

    /// Registers the actor with the actor system and starts it.
    /// This method will run the actor in a loop, handling messages and managing the actor's lifecycle.
    /// > Don't implement this method directly.
    /// > Instead, use the `register` method to register the actor with the actor system.
    async fn run_actor(
        &mut self,
        actor_system_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd>,
        error_handling: ErrorHandling,
        ready_tx: tokio::sync::mpsc::UnboundedSender<Result<(), ActorError>>,
    ) -> Result<(), ActorError> {
        let mut restarted = false;
        loop {
            if restarted {
                self.post_restart().await;
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message<Self>>();
            let (kill_tx, mut kill_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let (restart_tx, mut restart_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let mailbox = Arc::new(TypedMailbox::<Self>::new(tx.clone()));

            let mut count = 0;
            let result_rx = loop {
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = actor_system_tx.send(ActorSystemCmd::Register(
                    self.address().to_string(),
                    mailbox.clone(),
                    restart_tx.clone(),
                    kill_tx.clone(),
                    if restarted {
                        LifeCycle::Restarting
                    } else {
                        LifeCycle::Starting
                    },
                    result_tx,
                    restarted,
                )) {
                    count += 1;
                    error!(
                        "Failed to register actor {}...({}): {:?}",
                        self.address(),
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
                    return Err(ActorError::AddressAlreadyExist(self.address().to_string()));
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(ActorError::from(e)));
                    return Err(ActorError::UnhealthyActorSystem);
                }
                _ => {}
            }
            self.pre_start().await;
            restarted = true;
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Receiving,
            ));
            let _ = ready_tx.send(Ok(()));
            if let Some(_) = loop {
                tokio::select! {
                    Some(mut msg) = rx.recv() => {
                        let result_tx = msg.result_tx();
                        let msg_de = msg.inner();
                        match self.actor(msg_de).await {
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
                        info!("Kill actor: address={}", self.address());
                        break Some(());
                    }
                    Some(_) = restart_rx.recv() => {
                        info!("Restart actor: address={}", self.address());
                        break None;
                    }
                };
            } {
                let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                    self.address().to_string(),
                    LifeCycle::Stopping,
                ));
                self.post_stop().await;
                let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                    self.address().to_string(),
                    LifeCycle::Terminated,
                ));
                break Ok(());
            }
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Stopping,
            ));
            self.pre_restart().await;
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Restarting,
            ));
        }
    }

    /// Registers the actor with the actor system and starts it.
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
                    error!("Actor {} run failed: {:?}", self.address(), e);
                }
            })
        } else {
            tokio::spawn(async move {
                let result = self.run_actor(actor_system_tx, error_handling, tx).await;
                if let Err(e) = result {
                    error!("Actor {} run failed: {:?}", self.address(), e);
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
