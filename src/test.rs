use crate::{Actor, ActorError, ActorSystem, JobSpec};

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
        println!("[{}] got MyMessage1: {:?}", self.address(), msg);
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
        println!("[{}] got MyMessage2: {:?}", self.address(), msg);
        Ok(msg)
    }
}

#[tokio::test]
async fn test() {
    let mut actor_system = ActorSystem::new();

    let actor1 = MyActor1 {
        address: "/some/address/1/1".to_string(),
    };
    actor1.register(&mut actor_system, false).await;

    let actor2 = MyActor2 {
        address: "/some/address/2/1".to_string(),
    };
    actor2.register(&mut actor_system, false).await;

    let actor3 = MyActor1 {
        address: "/some/address/1/2".to_string(),
    };
    actor3.register(&mut actor_system, false).await;

    let _ = actor_system
        .send_broadcast::<MyActor1>(
            "/some/address/1/*".to_string(), /* address */
            MyMessage1::A("a1".to_string()), /* message */
        )
        .await;
    println!(
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
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let actor_system_move = actor_system.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        actor_system_move
            .send_and_recv::<MyActor1>(
                "/some/address/1/1".to_string(), /* address */
                MyMessage1::A("a2".to_string()), /* message */
            )
            .await
            .unwrap();
    });
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    actor_system
        .send_and_recv::<MyActor2>(
            "/some/address/2/1".to_string(), /* address */
            MyMessage2::B("b2".to_string()), /* message */
        )
        .await
        .unwrap();
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

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
            println!("result returned - {:?}", result);
        }
    }
    // kill and unregister actor
    actor_system.unregister("*".to_string() /* address as regex */);
}
