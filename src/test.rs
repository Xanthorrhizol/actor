#![cfg(feature = "xan-log")]
use crate::{Actor, ActorError, ActorSystem, Blocking, ErrorHandling, JobSpec};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MyMessage1 {
    A(String),
    C(String),
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum MyMessage2 {
    B(String),
}

#[derive(thiserror::Error, Debug)]
enum MyError {
    #[error("bye")]
    #[allow(dead_code)]
    Exit,
    #[error(transparent)]
    ActorError(#[from] ActorError),
}

struct MyActor1 {
    pub address: String,
}

struct MyActor2 {
    pub address: String,
}

#[async_trait::async_trait]
impl Actor for MyActor1 {
    type Message = MyMessage1;
    type Result = MyMessage1;
    type Error = MyError;

    fn address(&self) -> &str {
        &self.address
    }

    async fn actor(&mut self, msg: Self::Message) -> Result<Self::Result, Self::Error> {
        debug!("[{}] got MyMessage1: {:?}", self.address(), msg);
        Ok(msg)
    }
}

#[async_trait::async_trait]
impl Actor for MyActor2 {
    type Message = MyMessage2;
    type Result = MyMessage2;
    type Error = MyError;

    fn address(&self) -> &str {
        &self.address
    }

    async fn actor(&mut self, msg: Self::Message) -> Result<Self::Result, Self::Error> {
        debug!("[{}] got MyMessage2: {:?}", self.address(), msg);
        Ok(msg)
    }
}

#[tokio::test]
async fn test_with_tx_cache() {
    let _ = xan_log::init_logger();
    let mut actor_system = ActorSystem::new();

    let actor1 = MyActor1 {
        address: "/some/address/1/1".to_string(),
    };
    let _ = actor1
        .register(&mut actor_system, ErrorHandling::Stop, Blocking::Blocking)
        .await;

    let actor2 = MyActor2 {
        address: "/some/address/2/1".to_string(),
    };
    let _ = actor2
        .register(
            &mut actor_system,
            ErrorHandling::Stop,
            Blocking::NonBlocking,
        )
        .await;

    let actor3 = MyActor1 {
        address: "/some/address/1/2".to_string(),
    };
    let _ = actor3
        .register(&mut actor_system, ErrorHandling::Stop, Blocking::Blocking)
        .await;

    let actor3_duplicated = MyActor2 {
        address: "/some/address/1/2".to_string(),
    };
    info!(
        "[{}] test duplicated actor registration",
        actor3_duplicated.address(),
    );
    assert!(
        actor3_duplicated
            .register(
                &mut actor_system,
                ErrorHandling::Stop,
                Blocking::NonBlocking
            )
            .await
            .err()
            .is_some()
    );

    let _ = actor_system
        .send_broadcast::<MyActor1>(
            "/some/address/1/*".to_string(), /* address */
            MyMessage1::A("a1".to_string()), /* message */
        )
        .await;
    info!(
        "[{}] send_and_recv -> {:?}",
        "/some/address/2/1",
        actor_system
            .send_and_recv::<MyActor2>(
                "/some/address/2/1".to_string(), /* address */
                MyMessage2::B("b1".to_string()), /* message */
            )
            .await
            .unwrap()
    );

    // restart actor
    actor_system.restart("/some/address/1/*".to_string() /* address as regex */);

    let mut actor_system_move = actor_system.clone();
    tokio::spawn(async move {
        actor_system_move
            .send_and_recv::<MyActor1>(
                "/some/address/1/1".to_string(), /* address */
                MyMessage1::A("a2".to_string()), /* message */
            )
            .await
            .unwrap();
    });
    actor_system
        .send_and_recv::<MyActor2>(
            "/some/address/2/1".to_string(), /* address */
            MyMessage2::B("b2".to_string()), /* message */
        )
        .await
        .unwrap();

    let job = JobSpec::new(
        Some(2),                                 /* max_iter */
        Some(std::time::Duration::from_secs(3)), /* interval */
        std::time::SystemTime::now(),            /* start_at */
    );
    if let Ok(Some(mut recv_rx)) = actor_system
        .run_job::<MyActor1>(
            "/some/address/1/1".to_string(), /* address */
            true, /* whether subscribe the handler result or not(true => Some(rx)) */
            job,  /* job as JobSpec */
            MyMessage1::C("c".to_string()), /* message */
        )
        .await
    {
        while let Some(result) = recv_rx.recv().await {
            debug!("result returned - {:?}", result);
        }
    }
    // kill and unregister actor
    info!("kill and unregister actor *");
    actor_system.unregister("*".to_string() /* address as regex */);
}

#[tokio::test]
async fn test_without_tx_cache() {
    let _ = xan_log::init_logger();
    let mut actor_system = ActorSystem::new();

    let actor1 = MyActor1 {
        address: "/some/address/1/1".to_string(),
    };
    let _ = actor1
        .register(&mut actor_system, ErrorHandling::Stop, Blocking::Blocking)
        .await;

    let actor2 = MyActor2 {
        address: "/some/address/2/1".to_string(),
    };
    let _ = actor2
        .register(
            &mut actor_system,
            ErrorHandling::Stop,
            Blocking::NonBlocking,
        )
        .await;

    let actor3 = MyActor1 {
        address: "/some/address/1/2".to_string(),
    };
    let _ = actor3
        .register(&mut actor_system, ErrorHandling::Stop, Blocking::Blocking)
        .await;

    let actor3_duplicated = MyActor2 {
        address: "/some/address/1/2".to_string(),
    };
    info!(
        "[{}] test duplicated actor registration",
        actor3_duplicated.address(),
    );
    assert!(
        actor3_duplicated
            .register(
                &mut actor_system,
                ErrorHandling::Stop,
                Blocking::NonBlocking
            )
            .await
            .err()
            .is_some()
    );

    let _ = actor_system
        .send_broadcast_without_tx_cache::<MyActor1>(
            "/some/address/1/*".to_string(), /* address */
            MyMessage1::A("a1".to_string()), /* message */
        )
        .await;
    info!(
        "[{}] send_and_recv -> {:?}",
        "/some/address/2/1",
        actor_system
            .send_and_recv_without_tx_cache::<MyActor2>(
                "/some/address/2/1".to_string(), /* address */
                MyMessage2::B("b1".to_string()), /* message */
            )
            .await
            .unwrap()
    );

    // restart actor
    actor_system.restart("/some/address/1/*".to_string() /* address as regex */);

    let actor_system_move = actor_system.clone();
    tokio::spawn(async move {
        actor_system_move
            .send_and_recv_without_tx_cache::<MyActor1>(
                "/some/address/1/1".to_string(), /* address */
                MyMessage1::A("a2".to_string()), /* message */
            )
            .await
            .unwrap();
    });
    actor_system
        .send_and_recv_without_tx_cache::<MyActor2>(
            "/some/address/2/1".to_string(), /* address */
            MyMessage2::B("b2".to_string()), /* message */
        )
        .await
        .unwrap();

    let job = JobSpec::new(
        Some(2),                                 /* max_iter */
        Some(std::time::Duration::from_secs(3)), /* interval */
        std::time::SystemTime::now(),            /* start_at */
    );
    if let Ok(Some(mut recv_rx)) = actor_system
        .run_job_without_tx_cache::<MyActor1>(
            "/some/address/1/1".to_string(), /* address */
            true, /* whether subscribe the handler result or not(true => Some(rx)) */
            job,  /* job as JobSpec */
            MyMessage1::C("c".to_string()), /* message */
        )
        .await
    {
        while let Some(result) = recv_rx.recv().await {
            debug!("result returned - {:?}", result);
        }
    }
    // kill and unregister actor
    info!("kill and unregister actor *");
    actor_system.unregister("*".to_string() /* address as regex */);
}
