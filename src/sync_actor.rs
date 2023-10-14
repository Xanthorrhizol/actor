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
        std::sync::mpsc::Sender<Message<T, R>>,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Sender<()>,
        LifeCycle,
        std::sync::mpsc::Sender<()>,
    ),
    Restart(String),
    Unregister(String),
    FindActor(
        String,
        std::sync::mpsc::Sender<
            Option<(std::sync::mpsc::Sender<Message<T, R>>, bool)>, // tx, ready
        >,
    ),
    SetLifeCycle(String, LifeCycle),
}

pub trait Actor<T, R, E, P>
where
    Self: Sized + Send + Sync + 'static,
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
    E: Error + Send + 'static,
{
    fn address(&self) -> &str;

    fn new(params: P) -> Result<Self, E>;

    fn actor(&mut self, msg: T) -> Result<R, E>;

    fn pre_start(&mut self);

    fn pre_restart(&mut self);

    fn post_stop(&mut self);

    fn post_restart(&mut self);

    fn run_actor(
        &mut self,
        actor_system_tx: std::sync::mpsc::Sender<ActorSystemCmd<T, R>>,
        kill_in_error: bool,
        ready_tx: std::sync::mpsc::Sender<()>,
    ) -> Result<(), ActorError<T, R>> {
        let mut restarted = false;
        loop {
            if restarted {
                self.post_restart();
            }
            let (tx, rx) = std::sync::mpsc::channel::<Message<T, R>>();
            let (kill_tx, kill_rx) = std::sync::mpsc::channel::<()>();
            let (restart_tx, restart_rx) = std::sync::mpsc::channel::<()>();
            let (result_tx, result_rx) = std::sync::mpsc::channel::<()>();

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
            let _ = result_rx.recv();
            self.pre_start();
            restarted = true;
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Receiving,
            ));
            let _ = ready_tx.send(());
            if let Some(_) = loop {
                let result = rx.try_recv();
                let kill = kill_rx.try_recv();
                let restart = restart_rx.try_recv();
                if let Ok(()) = kill {
                    info!("Kill actor: address={}", self.address());
                    break Some(());
                }
                if let Ok(()) = restart {
                    info!("Restart actor: address={}", self.address());
                    break None;
                }
                if let Ok(msg) = result {
                    match self.actor(msg.inner()) {
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
                        }
                    }
                }
            } {
                let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                    self.address().to_string(),
                    LifeCycle::Stopping,
                ));
                self.post_stop();
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
            self.pre_restart();
            let _ = actor_system_tx.send(ActorSystemCmd::SetLifeCycle(
                self.address().to_string(),
                LifeCycle::Restarting,
            ));
        }
    }
    fn register(mut self, actor_system: &mut ActorSystem<T, R>, kill_in_error: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        let actor_system_tx = actor_system.handler_tx();
        let _ = std::thread::spawn(move || {
            debug!("starting handler...");
            self.run_actor(actor_system_tx, kill_in_error, tx)
        });
        let _ = rx.recv();
    }
}

pub struct ActorSystem<T: Clone + Send + Sync + 'static, R: Send + 'static> {
    _handler: Option<std::thread::JoinHandle<()>>,
    handler_tx: std::sync::mpsc::Sender<ActorSystemCmd<T, R>>,
}

impl<T, R> ActorSystem<T, R>
where
    T: Sized + Send + Clone + Sync + 'static,
    R: Sized + Send + 'static,
{
    pub fn new() -> Self {
        let (handler_tx, handler_rx) = std::sync::mpsc::channel();
        let mut me = Self {
            _handler: None,
            handler_tx,
        };
        me.run(handler_rx);
        me
    }

    pub fn handler_tx(&self) -> std::sync::mpsc::Sender<ActorSystemCmd<T, R>> {
        self.handler_tx.clone()
    }

    fn run(&mut self, handler_rx: std::sync::mpsc::Receiver<ActorSystemCmd<T, R>>) {
        self._handler = Some(std::thread::spawn(move || {
            let mut map = HashMap::<
                String,
                (
                    std::sync::mpsc::Sender<Message<T, R>>,
                    std::sync::mpsc::Sender<()>,
                    std::sync::mpsc::Sender<()>,
                    LifeCycle,
                ),
            >::new();
            while let Ok(msg) = handler_rx.recv() {
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
        }));
    }

    pub fn register(
        &mut self,
        address: String,
        tx: std::sync::mpsc::Sender<Message<T, R>>,
        restart_tx: std::sync::mpsc::Sender<()>,
        kill_tx: std::sync::mpsc::Sender<()>,
        life_cycle: LifeCycle,
    ) {
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let _ = self.handler_tx.send(ActorSystemCmd::Register(
            address, tx, restart_tx, kill_tx, life_cycle, result_tx,
        ));
        let _ = result_rx.recv();
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

    pub fn send(&self, address: String, msg: T) -> Result<(), ActorError<T, R>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.recv() {
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

    pub fn send_and_recv(&self, address: String, msg: T) -> Result<R, ActorError<T, R>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.recv() {
            if ready {
                let (result_tx, result_rx) = std::sync::mpsc::channel::<R>();
                let _ = tx.send(Message::new(msg, Some(result_tx)))?;
                Ok(result_rx.recv()?)
            } else {
                Err(ActorError::ActorNotReady(address))
            }
        } else {
            Err(ActorError::AddressNotFound(address))
        }
    }

    pub fn run_job(
        &self,
        address: String,
        subscript: bool,
        job: JobSpec,
        msg: T,
    ) -> Result<
        Option<std::sync::mpsc::Receiver<Result<R, std::sync::mpsc::RecvError>>>,
        ActorError<T, R>,
    > {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = self
            .handler_tx
            .send(ActorSystemCmd::FindActor(address.clone(), tx));
        if let Ok(Some((tx, ready))) = rx.recv() {
            if ready {
                let tx = tx.clone();
                if subscript {
                    let (sub_tx, sub_rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let mut i = 0;
                        if let Some(interval) = job.interval() {
                            loop {
                                i += 1;
                                if job.start_at() <= std::time::SystemTime::now() {
                                    let (result_tx, result_rx) = std::sync::mpsc::channel::<R>();
                                    let _ = tx.send(Message::new(msg.clone(), Some(result_tx)));
                                    let _ = sub_tx.send(result_rx.recv());
                                    std::thread::sleep(interval);
                                    if let Some(max_iter) = job.max_iter() {
                                        if i >= max_iter {
                                            break;
                                        }
                                    }
                                }
                            }
                        } else {
                            if job.start_at() <= std::time::SystemTime::now() {
                                let (result_tx, result_rx) = std::sync::mpsc::channel::<R>();
                                let _ = tx.send(Message::new(msg.clone(), Some(result_tx)));
                                let _ = sub_tx.send(result_rx.recv());
                            }
                        }
                    });
                    Ok(Some(sub_rx))
                } else {
                    std::thread::spawn(move || {
                        let mut i = 0;
                        if let Some(interval) = job.interval() {
                            loop {
                                i += 1;
                                if job.start_at() <= std::time::SystemTime::now() {
                                    let _ = tx.send(Message::new(msg.clone(), None));
                                    std::thread::sleep(interval);
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
