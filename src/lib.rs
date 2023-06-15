mod error;
pub mod types;

pub use crate::error::ActorError;
pub use crate::types::{JobSpec, Message};
use log::error;
use std::collections::HashMap;
use std::error::Error;

enum ActorSystemCmd<T, R>
where
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
{
    Register(
        String,
        tokio::sync::mpsc::UnboundedSender<Message<T, R>>,
        tokio::sync::mpsc::UnboundedSender<()>,
        LifeCycle,
    ),
    Unregister(String),
    FindActor(
        String,
        tokio::sync::oneshot::Sender<Option<tokio::sync::mpsc::UnboundedSender<Message<T, R>>>>,
    ),
    SetLifeCycle(String, LifeCycle),
}

#[derive(Clone)]
pub enum LifeCycle {
    Starting,
    Receiving,
    Stopping,
    Terminated,
    Restarting,
}

#[async_trait::async_trait]
pub trait Actor<T, R, E, P>
where
    Self: Sized + Send + Sync + 'static,
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
    E: Error + Send + 'static,
    P: Clone,
{
    fn address(&self) -> &str;

    async fn new(params: P) -> Result<Self, E>;

    async fn clone(&self) -> Result<Self, E>;

    async fn actor(&mut self, msg: T) -> Result<R, E>;

    async fn pre_start(&mut self, actor_system: &mut ActorSystem<T, R>);

    async fn pre_restart(&mut self, actor_system: &mut ActorSystem<T, R>);

    async fn post_stop(&mut self, actor_system: &mut ActorSystem<T, R>);

    async fn post_restart(&mut self, actor_system: &mut ActorSystem<T, R>);

    async fn run_actor(
        &mut self,
        actor_system: &mut ActorSystem<T, R>,
        kill_in_error: bool,
        ready_tx: tokio::sync::mpsc::UnboundedSender<()>,
    ) -> Result<(), ActorError<T, R>> {
        let mut restarted = false;
        loop {
            if restarted {
                self.post_restart(actor_system).await;
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message<T, R>>();
            let (kill_tx, mut kill_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
            let _ = actor_system.register(
                self.address().to_string(),
                tx,
                kill_tx,
                if restarted {
                    LifeCycle::Restarting
                } else {
                    LifeCycle::Starting
                },
            );
            self.pre_start(actor_system).await;
            let _ = ready_tx.send(());
            restarted = true;
            let mut self_clone = match self.clone().await {
                Ok(result) => result,
                Err(e) => {
                    error!(
                        "Clone actor failed: address={}, error={:?}",
                        self.address(),
                        e
                    );
                    Err(ActorError::CloneFailed(self.address().to_string()))?
                }
            };
            let _ = actor_system.set_lifecycle(self.address(), LifeCycle::Receiving);
            if let Ok(None) = tokio::spawn(async move {
                loop {
                    tokio::select! {
                       Some(msg) = rx.recv() => {
                           match self_clone.actor(msg.inner()).await {
                               Ok(result) => {
                                   if let Some(result_tx) = msg.result_tx() {
                                       let _ = result_tx.send(result);
                                   }
                               }
                               Err(e) => {
                                   error!("Handler's result has error: {}", e.to_string());
                                   if kill_in_error {
                                       break Some(e);
                                   }
                               }
                           }
                       }
                       Some(_) = kill_rx.recv() => {
                           break None;
                       }
                    };
                }
            })
            .await
            {
                let _ = actor_system.set_lifecycle(self.address(), LifeCycle::Stopping);
                self.post_stop(actor_system).await;
                let _ = actor_system.set_lifecycle(self.address(), LifeCycle::Terminated);
                break Ok(());
            }
            let _ = actor_system.set_lifecycle(self.address(), LifeCycle::Stopping);
            self.pre_restart(actor_system).await;
            let _ = actor_system.set_lifecycle(self.address(), LifeCycle::Restarting);
        }
    }
    async fn register(&mut self, actor_system: &mut ActorSystem<T, R>, kill_in_error: bool) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = self.run_actor(actor_system, kill_in_error, tx);
        let _ = rx.recv().await;
    }
}

pub struct ActorSystem<T: Clone + Send + Sync + 'static, R: Send + 'static> {
    _handler: Option<tokio::task::JoinHandle<()>>,
    handler_tx: tokio::sync::mpsc::UnboundedSender<ActorSystemCmd<T, R>>,
}

impl<T, R> ActorSystem<T, R>
where
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
{
    pub fn new() -> Self {
        let (handler_tx, handler_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut me = Self {
            _handler: None,
            handler_tx,
        };
        me.run(handler_rx);
        me
    }

    fn run(&mut self, mut handler_rx: tokio::sync::mpsc::UnboundedReceiver<ActorSystemCmd<T, R>>) {
        self._handler = Some(tokio::spawn(async move {
            let mut map = HashMap::<
                String,
                (
                    tokio::sync::mpsc::UnboundedSender<Message<T, R>>,
                    tokio::sync::mpsc::UnboundedSender<()>,
                    LifeCycle,
                ),
            >::new();
            while let Some(msg) = handler_rx.recv().await {
                match msg {
                    ActorSystemCmd::Register(address, tx, kill_tx, life_cycle) => {
                        map.insert(address, (tx, kill_tx, life_cycle));
                    }
                    ActorSystemCmd::Unregister(address) => {
                        if let Some((_tx, kill_tx, _life_cycle)) = map.get(&address) {
                            let _ = kill_tx.send(());
                            map.remove(&address);
                        }
                    }
                    ActorSystemCmd::FindActor(address, result_tx) => {
                        if let Some((tx, _kill_tx, _life_cycle)) = map.get(&address) {
                            let _ = result_tx.send(Some(tx.clone()));
                        } else {
                            let _ = result_tx.send(None);
                        }
                    }
                    ActorSystemCmd::SetLifeCycle(address, life_cycle) => {
                        if let Some(actor) = map.get_mut(&address) {
                            actor.2 = life_cycle;
                        };
                    }
                };
            }
        }));
    }

    pub fn register(
        &mut self,
        address: String,
        tx: tokio::sync::mpsc::UnboundedSender<Message<T, R>>,
        kill_tx: tokio::sync::mpsc::UnboundedSender<()>,
        life_cycle: LifeCycle,
    ) {
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::Register(address, tx, kill_tx, life_cycle));
    }

    pub fn set_lifecycle(&mut self, address: &str, life_cycle: LifeCycle) {
        let _ = self.handler_tx.send(ActorSystemCmd::SetLifeCycle(
            address.to_string(),
            life_cycle,
        ));
    }

    pub fn unregister(&mut self, address: String) {
        let _ = self.handler_tx.send(ActorSystemCmd::Unregister(address));
    }

    pub async fn send(&self, address: String, msg: T) -> Result<(), ActorError<T, R>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some(tx)) = rx.await {
            let _ = tx.send(Message::new(msg, None))?;
            Ok(())
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    pub async fn send_and_recv(&self, address: String, msg: T) -> Result<R, ActorError<T, R>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some(tx)) = rx.await {
            let (result_tx, result_rx) = tokio::sync::oneshot::channel::<R>();
            let _ = tx.send(Message::new(msg, Some(result_tx)))?;
            Ok(result_rx.await?)
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
        if let Ok(Some(tx)) = rx.await {
            let tx = tx.clone();
            if subscript {
                let (sub_tx, sub_rx) = tokio::sync::mpsc::unbounded_channel();
                tokio::spawn(async move {
                    let mut i = 0;
                    if let Some(interval) = job.interval() {
                        loop {
                            i += 1;
                            if job.start_at() <= std::time::SystemTime::now() {
                                let (result_tx, result_rx) = tokio::sync::oneshot::channel::<R>();
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
            Err(ActorError::AddressNotFound(address))
        }
    }
}
