use crate::{error::ActorError, Actor, ActorSystem};

#[derive(Clone, Debug)]
pub enum MyMessage {
    A(String),
    B(String),
    Exit,
}

#[derive(thiserror::Error, Debug)]
enum MyError<T, R>
where
    T: Sized + Send + Clone,
    R: Sized + Send,
{
    #[error("bye")]
    Exit,
    #[error(transparent)]
    ActorError(#[from] ActorError<T, R>),
}

struct MyActor {
    address: String,
}

#[async_trait::async_trait]
impl Actor<MyMessage, (), MyError<MyMessage, ()>, String> for MyActor
where
    Self: Send + Sized + Sync + 'static,
{
    fn address(&self) -> &str {
        &self.address
    }

    async fn new(params: String) -> Result<Self, MyError<MyMessage, ()>> {
        Ok(Self { address: params })
    }

    async fn actor(&mut self, msg: MyMessage) -> Result<(), MyError<MyMessage, ()>> {
        match msg {
            MyMessage::A(s) => {
                println!("got A: {}", s);
            }
            MyMessage::B(s) => {
                println!("got B: {}", s);
            }
            MyMessage::Exit => {
                println!("got Exit");
                return Err(MyError::Exit);
            }
        }
        Ok(())
    }

    async fn pre_start(&mut self) {}
    async fn pre_restart(&mut self) {}
    async fn post_stop(&mut self) {}
    async fn post_restart(&mut self) {}
}

#[tokio::test]
async fn test() {
    let mut actor_system = ActorSystem::new();
    let actor = MyActor::new("some-address".to_string()).await.unwrap();
    actor.register(&mut actor_system, false).await;

    let _ = actor_system.send(
        "some-address".to_string(),    /* address */
        MyMessage::A("a".to_string()), /* message */
    );
    let result = actor_system
        .send_and_recv(
            "some-address".to_string(),    /* address */
            MyMessage::B("b".to_string()), /* message */
        )
        .await;
    println!("{:?}", result);

    // restart actor
    actor_system.restart("some-address".to_string() /* address */);
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    let result = actor_system
        .send_and_recv(
            "some-address".to_string(),    /* address */
            MyMessage::A("a".to_string()), /* message */
        )
        .await;
    println!("{:?}", result);

    // kill and unregister actor
    actor_system.unregister("some-address".to_string() /* address */);
}
