use crate::error::ActorError;
use crate::types::{JobSpec, Message};
use crate::LifeCycle;
use std::collections::HashMap;
use std::error::Error;

pub enum ActorSystemCmd<T, R>
where
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
{
    Register(
        String,
        tokio::sync::mpsc::UnboundedSender<Message<T, R>>,
        tokio::sync::mpsc::UnboundedSender<()>,
        tokio::sync::mpsc::UnboundedSender<()>,
        LifeCycle,
        tokio::sync::oneshot::Sender<()>,
    ),
    Restart(String),
    Unregister(String),
    FindActor(
        String,
        tokio::sync::oneshot::Sender<
            Option<(tokio::sync::mpsc::UnboundedSender<Message<T, R>>, bool)>, // tx, ready
        >,
    ),
    SetLifeCycle(String, LifeCycle),
}

#[async_trait::async_trait]
pub trait Actor<T, R, E, P>
where
    Self: Sized + Send + Sync + 'static,
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
    E: Error + Send + 'static,
{
    fn address(&self) -> &str;

    async fn new(params: P) -> Result<Self, E>;

    async fn actor(&mut self, msg: T) -> Result<R, E>;

    async fn pre_start(&mut self);

    async fn pre_restart(&mut self);

    async fn post_stop(&mut self);

    async fn post_restart(&mut self);

    async fn run_actor(
        &mut self,
        actor_system_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd<T, R>>,
        kill_in_error: bool,
        ready_tx: tokio::sync::mpsc::UnboundedSender<()>,
    ) -> Result<(), ActorError<T, R>> {
        let mut restarted = false;
        loop {
            if restarted {
                self.post_restart().await;
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message<T, R>>();
            let (kill_tx, mut kill_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let (restart_tx, mut restart_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();

            let _ = actor_system_tx.send(ActorSystemCmd::Register(
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
            ));
            let _ = result_rx.await;
            self.pre_start().await;
            restarted = true;
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Receiving,
            ));
            let _ = ready_tx.send(());
            if let Some(_) = loop {
                tokio::select! {
                    Some(msg) = rx.recv() => {
                        match self.actor(msg.inner()).await {
                           Ok(result) => {
                                if let Some(result_tx) = msg.result_tx() {
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
    async fn register(mut self, actor_system: &mut ActorSystem<T, R>, kill_in_error: bool) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let actor_system_tx = actor_system.handler_tx();
        let _ =
            tokio::spawn(async move { self.run_actor(actor_system_tx, kill_in_error, tx).await });
        let _ = rx.recv().await;
    }
}

#[derive(Clone)]
pub struct ActorSystem<T: Clone + Send + Sync + 'static, R: Send + 'static> {
    handler_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd<T, R>>,
}

impl<T, R> ActorSystem<T, R>
where
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
{
    pub fn new() -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut me = Self { handler_tx };
        me.run(handler_rx);
        me
    }

    pub fn handler_tx(&self) -> tokio::sync::mpsc::UnboundedSender<ActorSystemCmd<T, R>> {
        self.handler_tx.clone()
    }

    fn run(
        &mut self,
        mut handler_rx: tokio::sync::mpsc::UnboundedReceiver<ActorSystemCmd<T, R>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut map = HashMap::<
                String,
                (
                    tokio::sync::mpsc::UnboundedSender<Message<T, R>>,
                    tokio::sync::mpsc::UnboundedSender<()>,
                    tokio::sync::mpsc::UnboundedSender<()>,
                    LifeCycle,
                ),
            >::new();
            while let Some(msg) = handler_rx.recv().await {
                match msg {
                    ActorSystemCmd::Register(
                        address,
                        tx,
                        restart_tx,
                        kill_tx,
                        life_cycle,
                        result_tx,
                    ) => {
                        debug!("Register actor with address {}", address);
                        map.insert(address.clone(), (tx, restart_tx, kill_tx, life_cycle));
                        let _ = result_tx.send(());
                    }
                    ActorSystemCmd::Restart(address) => {
                        debug!("Restart actor with address {}", address);

                        if let Some((_tx, restart_tx, _kill_tx, _life_cycle)) = map.get(&address) {
                            let _ = restart_tx.send(());
                        }
                    }
                    ActorSystemCmd::Unregister(address) => {
                        debug!("Unregister actor with address {}", address);
                        if let Some((_tx, _restart_tx, kill_tx, _life_cycle)) = map.get(&address) {
                            let _ = kill_tx.send(());
                            map.remove(&address);
                        }
                    }
                    ActorSystemCmd::FindActor(address, result_tx) => {
                        debug!("FindActor with address {}", address);
                        if let Some((tx, _restart_tx, _kill_tx, life_cycle)) = map.get(&address) {
                            let _ = result_tx.send(Some((
                                tx.clone(),
                                match life_cycle {
                                    LifeCycle::Receiving => true,
                                    _ => false,
                                },
                            )));
                        } else {
                            let _ = result_tx.send(None);
                        }
                    }
                    ActorSystemCmd::SetLifeCycle(address, life_cycle) => {
                        debug!(
                            "SetLifecycle with address {} into {:?}",
                            address, life_cycle
                        );
                        if let Some(actor) = map.get_mut(&address) {
                            actor.3 = life_cycle;
                        };
                    }
                };
            }
        })
    }

    pub async fn register(
        &mut self,
        address: String,
        tx: tokio::sync::mpsc::UnboundedSender<Message<T, R>>,
        restart_tx: tokio::sync::mpsc::UnboundedSender<()>,
        kill_tx: tokio::sync::mpsc::UnboundedSender<()>,
        life_cycle: LifeCycle,
    ) {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::Register(
            address, tx, restart_tx, kill_tx, life_cycle, result_tx,
        ));
        let _ = result_rx.await;
    }

    pub fn set_lifecycle(&mut self, address: &str, life_cycle: LifeCycle) {
        let _ = self.handler_tx.send(ActorSystemCmd::SetLifeCycle(
            address.to_string(),
            life_cycle,
        ));
    }

    pub fn restart(&mut self, address: String) {
        let _ = self.handler_tx.send(ActorSystemCmd::Restart(address));
    }

    pub fn unregister(&mut self, address: String) {
        let _ = self.handler_tx.send(ActorSystemCmd::Unregister(address));
    }

    pub async fn send(&self, address: String, msg: T) -> Result<(), ActorError<T, R>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.await {
            if ready {
                let _ = tx.send(Message::new(msg, None))?;
                Ok(())
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    pub async fn send_and_recv(&self, address: String, msg: T) -> Result<R, ActorError<T, R>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.await {
            if ready {
                let (result_tx, result_rx) = tokio::sync::oneshot::channel::<R>();
                let _ = tx.send(Message::new(msg, Some(result_tx)))?;
                Ok(result_rx.await?)
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    pub async fn run_job(
        &self,
        address: String,
        subscript: bool,
        job: JobSpec,
        msg: T,
    ) -> Result<
        Option<
            tokio::sync::mpsc::UnboundedReceiver<Result<R, tokio::sync::oneshot::error::RecvError>>,
        >,
        ActorError<T, R>,
    > {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.await {
            if ready {
                let tx = tx.clone();
                if subscript {
                    let (sub_tx, sub_rx) = tokio::sync::mpsc::unbounded_channel();
                    tokio::spawn(async move {
                        let mut i = 0;
                        if let Some(interval) = job.interval() {
                            loop {
                                i += 1;
                                if job.start_at() <= std::time::SystemTime::now() {
                                    let (result_tx, result_rx) =
                                        tokio::sync::oneshot::channel::<R>();
                                    let _ = tx.send(Message::new(msg.clone(), Some(result_tx)));
                                    let _ = sub_tx.send(result_rx.await);
                                    tokio::time::sleep(interval).await;
                                    if let Some(max_iter) = job.max_iter() {
                                        if i >= max_iter {
                                            break;
                                        }
                                    }
                                }
                            }
                        } else {
                            if job.start_at() <= std::time::SystemTime::now() {
                                let (result_tx, result_rx) = tokio::sync::oneshot::channel::<R>();
                                let _ = tx.send(Message::new(msg.clone(), Some(result_tx)));
                                let _ = sub_tx.send(result_rx.await);
                            }
                        }
                    });
                    Ok(Some(sub_rx))
                } else {
                    tokio::spawn(async move {
                        let mut i = 0;
                        if let Some(interval) = job.interval() {
                            loop {
                                i += 1;
                                if job.start_at() <= std::time::SystemTime::now() {
                                    let _ = tx.send(Message::new(msg.clone(), None));
                                    tokio::time::sleep(interval).await;
                                    if let Some(max_iter) = job.max_iter() {
                                        if i >= max_iter {
                                            break;
                                        }
                                    }
                                }
                            }
                        } else {
                            if job.start_at() <= std::time::SystemTime::now() {
                                let _ = tx.send(Message::new(msg.clone(), None));
                            }
                        }
                    });
                    Ok(None)
                }
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }
}
