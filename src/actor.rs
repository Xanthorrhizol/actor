use crate::{ActorError, ActorSystem, ActorSystemCmd, LifeCycle, Message};
use std::error::Error;

#[async_trait::async_trait]
pub trait Actor
where
    Self: Sized + 'static,
{
    type Message: Sized + Send + serde::Serialize + serde::de::DeserializeOwned;

    type Result: Sized + Send + serde::Serialize + serde::de::DeserializeOwned;

    type Error: Error + Send;

    fn address(&self) -> &str;

    async fn actor(&mut self, msg: Self::Message) -> Result<Self::Result, Self::Error>;

    async fn pre_start(&mut self) {}

    async fn pre_restart(&mut self) {}

    async fn post_stop(&mut self) {}

    async fn post_restart(&mut self) {}

    async fn run_actor(
        &mut self,
        actor_system_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd>,
        kill_in_error: bool,
        ready_tx: tokio::sync::mpsc::UnboundedSender<Result<(), ActorError>>,
    ) -> Result<(), ActorError> {
        let mut restarted = false;
        loop {
            if restarted {
                self.post_restart().await;
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
            let (kill_tx, mut kill_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let (restart_tx, mut restart_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();

            if actor_system_tx
                .send(ActorSystemCmd::Register(
                    self.address().to_string(),
                    tx,
                    restart_tx,
                    kill_tx,
                    if restarted {
                        LifeCycle::Restarting
                    } else {
                        LifeCycle::Starting
                    },
                    result_tx,
                    restarted,
                ))
                .is_err()
            {
                let _ = ready_tx.send(Err(ActorError::UnhealthyActorSystem));
                return Err(ActorError::UnhealthyActorSystem);
            }
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
                        let msg_de = match rmp_serde::from_slice::<Self::Message>(msg.inner()) {
                            Ok(msg) => msg,
                            Err(e) => {
                                if kill_in_error {
                                    error!("Deserialize message failed: {:?}", e);
                                    break Some(());
                                }
                                debug!("Deserialize message failed: {:?}", e);
                                break None;
                            }
                        };
                        match self.actor(msg_de).await {
                           Ok(result) => {
                                if let Some(result_tx) = result_tx {
                                    let result = rmp_serde::to_vec(&result)?;
                                    let _ = result_tx.send(result);
                                }
                            }
                           Err(e) => {
                                if kill_in_error {
                                    error!("Handler's result has error: {:?}", e);
                                    break Some(());
                                }
                                debug!("Handler's result has error: {:?}", e);
                                break None;
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
    async fn register(
        mut self,
        actor_system: &mut ActorSystem,
        kill_in_error: bool,
    ) -> Result<(), ActorError> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let actor_system_tx = actor_system.handler_tx();
        let _ = tokio::task::spawn_blocking(move || {
            let result = tokio::runtime::Handle::current().block_on(self.run_actor(
                actor_system_tx,
                kill_in_error,
                tx,
            ));
            if let Err(e) = result {
                error!("Actor {} run failed: {:?}", self.address(), e);
            }
        });
        if let Some(result) = rx.recv().await {
            result
        } else {
            Err(ActorError::UnboundedChannelRecv)
        }
    }
}
