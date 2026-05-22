#![cfg(all(feature = "multi-node", feature = "unbounded-channel"))]

use crate::inter_node::{Address, NodeFilter, Topic};
use crate::{Actor, ActorError, ActorSystem, Blocking, ErrorHandling, JobSpec, RunJobResult};
use std::sync::Arc;
use xancode::Codec;
use xanq::server::Server;

#[derive(Debug, Clone, Codec)]
pub enum RemoteMsg {
    Ping(String),
    Echo(String),
    Bye,
}

#[derive(thiserror::Error, Debug)]
enum TestError {
    #[error(transparent)]
    Actor(#[from] ActorError),
}

struct RemoteActor {
    addr: Address,
}

#[async_trait::async_trait]
impl Actor for RemoteActor {
    type Message = RemoteMsg;
    type Result = RemoteMsg;
    type Error = TestError;

    fn address(&self) -> &Address {
        &self.addr
    }

    async fn handle(&mut self, msg: Arc<Self::Message>) -> Result<Self::Result, Self::Error> {
        let response = match &*msg {
            RemoteMsg::Ping(s) => RemoteMsg::Echo(format!("pong:{s}")),
            RemoteMsg::Echo(s) => RemoteMsg::Echo(s.clone()),
            RemoteMsg::Bye => RemoteMsg::Bye,
        };
        Ok(response)
    }
}

crate::register_for_inter_node!(RemoteActor);

async fn spawn_broker() -> String {
    let (server, addr) = Server::<Topic>::spawn("127.0.0.1:0")
        .await
        .expect("spawn xanq broker");
    std::mem::forget(server);
    addr.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_and_recv_across_nodes() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new("ub-node-b".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("ub-node-b", "/remote/echo"),
    }
    .register(&mut node_b, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect("register remote actor");

    let mut node_a = ActorSystem::new("ub-node-a".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    let result = node_a
        .send_and_recv::<RemoteActor>(
            Address::new("ub-node-b", "/remote/echo"),
            RemoteMsg::Ping("hi".into()),
        )
        .await
        .expect("send_and_recv across nodes");

    match result {
        RemoteMsg::Echo(s) => assert_eq!(s, "pong:hi"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fire_across_nodes() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new("ub-node-b2".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("ub-node-b2", "/remote/sink"),
    }
    .register(&mut node_b, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect("register remote actor");

    let mut node_a = ActorSystem::new("ub-node-a2".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    node_a
        .send::<RemoteActor>(Address::new("ub-node-b2", "/remote/sink"), RemoteMsg::Bye)
        .await
        .expect("fire across nodes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_routing_when_address_node_matches_self() {
    let broker = spawn_broker().await;

    let mut system = ActorSystem::new("ub-solo".into(), Some(broker.clone()))
        .await
        .expect("system connect");

    RemoteActor {
        addr: Address::new("ub-solo", "/local/only"),
    }
    .register(&mut system, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect("register local actor");

    let result = system
        .send_and_recv::<RemoteActor>(
            Address::new("ub-solo", "/local/only"),
            RemoteMsg::Ping("local".into()),
        )
        .await
        .expect("local routing");

    match result {
        RemoteMsg::Echo(s) => assert_eq!(s, "pong:local"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn register_rejected_when_address_node_doesnt_match_self() {
    let broker = spawn_broker().await;

    let mut node_a = ActorSystem::new("ub-node-a4".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    let err = RemoteActor {
        addr: Address::new("ub-node-b4", "/foreign/echo"),
    }
    .register(&mut node_a, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect_err("foreign-node address should be rejected");

    match err {
        ActorError::AddressNotOwned(s) => assert_eq!(s, "ub-node-b4:/foreign/echo"),
        other => panic!("expected AddressNotOwned, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_tx_cache_variants_route_remotely() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new("ub-node-b5".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("ub-node-b5", "/remote/nocache"),
    }
    .register(&mut node_b, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect("register remote actor");

    let node_a = ActorSystem::new("ub-node-a5".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    node_a
        .send_without_tx_cache::<RemoteActor>(
            Address::new("ub-node-b5", "/remote/nocache"),
            RemoteMsg::Bye,
        )
        .await
        .expect("send_without_tx_cache across nodes");

    let result = node_a
        .send_and_recv_without_tx_cache::<RemoteActor>(
            Address::new("ub-node-b5", "/remote/nocache"),
            RemoteMsg::Ping("nc".into()),
        )
        .await
        .expect("send_and_recv_without_tx_cache across nodes");

    match result {
        RemoteMsg::Echo(s) => assert_eq!(s, "pong:nc"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broadcast_fans_out_local_and_remote() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new("ub-node-b6".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("ub-node-b6", "/bc/remote/1"),
    }
    .register(&mut node_b, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect("register remote 1");
    RemoteActor {
        addr: Address::new("ub-node-b6", "/bc/remote/2"),
    }
    .register(&mut node_b, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect("register remote 2");

    let mut node_a = ActorSystem::new("ub-node-a6".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");
    RemoteActor {
        addr: Address::new("ub-node-a6", "/bc/local/1"),
    }
    .register(&mut node_a, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect("register local");

    let results = node_a
        .send_broadcast::<RemoteActor>(
            "/bc/.*".into(),
            NodeFilter::Peers(vec!["ub-node-a6".into(), "ub-node-b6".into()]),
            RemoteMsg::Bye,
        )
        .await;

    // 1 remote BroadcastFire envelope (to ub-node-b6) + 1 local match (/bc/local/1).
    assert_eq!(results.len(), 2, "expected 2 entries, got {:?}", results);
    for r in &results {
        assert!(r.is_ok(), "fan-out element failed: {:?}", r);
    }
}

/// Mirror of `test_unbounded::test_with_tx_cache` adapted for the multi-node
/// `Address` API. Verifies the local-routing fast path under the unbounded
/// `multi-node` configuration.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn basic_behavior_with_tx_cache() {
    let broker = spawn_broker().await;

    let mut peer = ActorSystem::new("ub-basic-c-peer".into(), Some(broker.clone()))
        .await
        .expect("peer system");
    RemoteActor {
        addr: Address::new("ub-basic-c-peer", "/a/3"),
    }
    .register(&mut peer, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("/a/3 on peer");

    let mut sys = ActorSystem::new("ub-basic-c".into(), Some(broker.clone()))
        .await
        .expect("system");

    RemoteActor {
        addr: Address::new("ub-basic-c", "/a/1"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("/a/1");
    RemoteActor {
        addr: Address::new("ub-basic-c", "/a/2"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("/a/2");
    RemoteActor {
        addr: Address::new("ub-basic-c", "/b/1"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("/b/1");

    let dup_err = RemoteActor {
        addr: Address::new("ub-basic-c", "/a/1"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect_err("duplicate address must be rejected");
    assert!(matches!(dup_err, ActorError::AddressAlreadyExist(_)));

    // Broadcast across both nodes: 2 local + 1 BroadcastFire to peer.
    let bc = sys
        .send_broadcast::<RemoteActor>(
            "/a/*".into(),
            NodeFilter::Peers(vec!["ub-basic-c".into(), "ub-basic-c-peer".into()]),
            RemoteMsg::Bye,
        )
        .await;
    assert_eq!(
        bc.len(),
        3,
        "expected 2 local fan-out + 1 remote BroadcastFire, got {:?}",
        bc
    );
    for r in &bc {
        assert!(r.is_ok());
    }

    let resp = sys
        .send_and_recv::<RemoteActor>(
            Address::new("ub-basic-c", "/b/1"),
            RemoteMsg::Ping("hello".into()),
        )
        .await
        .expect("send_and_recv");
    assert!(matches!(resp, RemoteMsg::Echo(ref s) if s == "pong:hello"));

    sys.restart("/a/*".into());
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let resp = sys
        .send_and_recv::<RemoteActor>(
            Address::new("ub-basic-c", "/a/1"),
            RemoteMsg::Ping("after-restart".into()),
        )
        .await
        .expect("send_and_recv after restart");
    assert!(matches!(resp, RemoteMsg::Echo(ref s) if s == "pong:after-restart"));

    let mut sys_clone = sys.clone();
    let resp = sys_clone
        .send_and_recv::<RemoteActor>(
            Address::new("ub-basic-c", "/b/1"),
            RemoteMsg::Ping("from-clone".into()),
        )
        .await
        .expect("send_and_recv from clone");
    assert!(matches!(resp, RemoteMsg::Echo(ref s) if s == "pong:from-clone"));

    let job = JobSpec::new(
        Some(2),
        Some(std::time::Duration::from_millis(50)),
        std::time::SystemTime::now(),
    );
    let RunJobResult {
        job_id: _,
        result_subscriber_rx,
    } = sys
        .run_job::<RemoteActor>(
            Address::new("ub-basic-c", "/a/1"),
            true,
            job,
            RemoteMsg::Ping("job".into()),
            None,
        )
        .await
        .expect("run_job");
    let mut rx = result_subscriber_rx.expect("subscriber");
    let mut got = 0usize;
    while let Some(item) = rx.recv().await {
        item.expect("iter ok");
        got += 1;
    }
    assert_eq!(got, 2);

    let job = JobSpec::new(
        None,
        Some(std::time::Duration::from_millis(50)),
        std::time::SystemTime::now(),
    );
    let RunJobResult {
        job_id,
        result_subscriber_rx,
    } = sys
        .run_job::<RemoteActor>(
            Address::new("ub-basic-c", "/a/1"),
            true,
            job,
            RemoteMsg::Ping("forever".into()),
            None,
        )
        .await
        .expect("run_job infinite");
    let mut rx = result_subscriber_rx.expect("subscriber");
    let mut got = 0usize;
    while let Some(item) = rx.recv().await {
        item.expect("iter ok");
        got += 1;
        if got == 2 {
            sys.stop_job(job_id.clone()).await;
            break;
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    sys.resume_job(job_id.clone()).await;
    got = 0;
    while let Some(item) = rx.recv().await {
        item.expect("iter ok after resume");
        got += 1;
        if got == 2 {
            sys.abort_job(job_id).await;
            break;
        }
    }

    sys.unregister("*".into());
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let err = sys
        .send_and_recv::<RemoteActor>(
            Address::new("ub-basic-c", "/b/1"),
            RemoteMsg::Ping("after-unregister".into()),
        )
        .await
        .expect_err("send after unregister must fail");
    let _ = err;
}

/// Mirror of `test_unbounded::test_without_tx_cache` adapted for the
/// multi-node `Address` API. Exercises the `_without_tx_cache` variants on
/// local addresses.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn basic_behavior_without_tx_cache() {
    let broker = spawn_broker().await;

    let mut peer = ActorSystem::new("ub-basic-nc-peer".into(), Some(broker.clone()))
        .await
        .expect("peer system");
    RemoteActor {
        addr: Address::new("ub-basic-nc-peer", "/a/9"),
    }
    .register(&mut peer, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("/a/9 on peer");

    let mut sys = ActorSystem::new("ub-basic-nc".into(), Some(broker.clone()))
        .await
        .expect("system");

    RemoteActor {
        addr: Address::new("ub-basic-nc", "/a/1"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("/a/1");
    RemoteActor {
        addr: Address::new("ub-basic-nc", "/a/2"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("/a/2");

    let bc = sys
        .send_broadcast_without_tx_cache::<RemoteActor>(
            "/a/*".into(),
            NodeFilter::Peers(vec!["ub-basic-nc".into(), "ub-basic-nc-peer".into()]),
            RemoteMsg::Bye,
        )
        .await;
    assert_eq!(bc.len(), 3, "expected 2 local + 1 remote, got {:?}", bc);
    assert!(bc.iter().all(|r| r.is_ok()));

    let resp = sys
        .send_and_recv_without_tx_cache::<RemoteActor>(
            Address::new("ub-basic-nc", "/a/2"),
            RemoteMsg::Ping("nc".into()),
        )
        .await
        .expect("send_and_recv_without_tx_cache");
    assert!(matches!(resp, RemoteMsg::Echo(ref s) if s == "pong:nc"));

    sys.send_without_tx_cache::<RemoteActor>(
        Address::new("ub-basic-nc", "/a/1"),
        RemoteMsg::Bye,
    )
    .await
    .expect("send_without_tx_cache");

    let job = JobSpec::new(
        Some(3),
        Some(std::time::Duration::from_millis(50)),
        std::time::SystemTime::now(),
    );
    let RunJobResult {
        job_id: _,
        result_subscriber_rx,
    } = sys
        .run_job_without_tx_cache::<RemoteActor>(
            Address::new("ub-basic-nc", "/a/1"),
            true,
            job,
            RemoteMsg::Ping("nc-job".into()),
            None,
        )
        .await
        .expect("run_job_without_tx_cache");
    let mut rx = result_subscriber_rx.expect("subscriber");
    let mut got = 0usize;
    while let Some(item) = rx.recv().await {
        item.expect("iter ok");
        got += 1;
    }
    assert_eq!(got, 3);
}

/// Cross-node throughput sanity check using the cached `send_and_recv` path.
/// Mirrors `test_unbounded::test_bench_with_tx_cache` but every iteration
/// round trips through the broker.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bench_send_and_recv_across_nodes_with_tx_cache() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new("ub-bench-b".into(), Some(broker.clone()))
        .await
        .expect("node-b");
    RemoteActor {
        addr: Address::new("ub-bench-b", "/bench/echo"),
    }
    .register(&mut node_b, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("register bench actor");

    let mut node_a = ActorSystem::new("ub-bench-a".into(), Some(broker.clone()))
        .await
        .expect("node-a");

    let payload = "x".repeat(1024);
    let target = Address::new("ub-bench-b", "/bench/echo");
    let now = std::time::Instant::now();
    const ITERS: usize = 1000;
    for _ in 0..ITERS {
        let _ = node_a
            .send_and_recv::<RemoteActor>(target.clone(), RemoteMsg::Ping(payload.clone()))
            .await
            .expect("send_and_recv");
    }
    let elapsed = now.elapsed();
    println!(
        "[bench] cross-node send_and_recv with_tx_cache (unbounded): {} iters, {} ms ({:.2} ms/op)",
        ITERS,
        elapsed.as_millis(),
        elapsed.as_secs_f64() * 1000.0 / ITERS as f64
    );
}

/// Same bench but via `send_and_recv_without_tx_cache` (unbounded variant).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bench_send_and_recv_across_nodes_without_tx_cache() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new("ub-bench-b-nc".into(), Some(broker.clone()))
        .await
        .expect("node-b");
    RemoteActor {
        addr: Address::new("ub-bench-b-nc", "/bench/echo"),
    }
    .register(&mut node_b, ErrorHandling::Stop, Blocking::NonBlocking)
    .await
    .expect("register bench actor");

    let node_a = ActorSystem::new("ub-bench-a-nc".into(), Some(broker.clone()))
        .await
        .expect("node-a");

    let payload = "x".repeat(1024);
    let target = Address::new("ub-bench-b-nc", "/bench/echo");
    let now = std::time::Instant::now();
    const ITERS: usize = 1000;
    for _ in 0..ITERS {
        let _ = node_a
            .send_and_recv_without_tx_cache::<RemoteActor>(
                target.clone(),
                RemoteMsg::Ping(payload.clone()),
            )
            .await
            .expect("send_and_recv_without_tx_cache");
    }
    let elapsed = now.elapsed();
    println!(
        "[bench] cross-node send_and_recv without_tx_cache (unbounded): {} iters, {} ms ({:.2} ms/op)",
        ITERS,
        elapsed.as_millis(),
        elapsed.as_secs_f64() * 1000.0 / ITERS as f64
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_job_against_remote_actor() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new("ub-node-b7".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("ub-node-b7", "/remote/job"),
    }
    .register(&mut node_b, ErrorHandling::Resume, Blocking::NonBlocking)
    .await
    .expect("register remote job actor");

    let mut node_a = ActorSystem::new("ub-node-a7".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    let job = JobSpec::new(
        Some(3),
        Some(std::time::Duration::from_millis(50)),
        std::time::SystemTime::now(),
    );

    let RunJobResult {
        job_id: _,
        result_subscriber_rx,
    } = node_a
        .run_job::<RemoteActor>(
            Address::new("ub-node-b7", "/remote/job"),
            true,
            job,
            RemoteMsg::Ping("job".into()),
            None,
        )
        .await
        .expect("run_job against remote");

    let mut rx = result_subscriber_rx.expect("subscribe=true should yield rx");
    let mut received = 0usize;
    while let Some(item) = rx.recv().await {
        match item.expect("job iteration ok") {
            RemoteMsg::Echo(s) => assert_eq!(s, "pong:job"),
            other => panic!("unexpected result: {other:?}"),
        }
        received += 1;
    }
    assert_eq!(received, 3, "expected 3 job iterations, got {received}");
}
