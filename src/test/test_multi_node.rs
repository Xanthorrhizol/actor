#![cfg(all(feature = "multi-node", feature = "bounded-channel"))]

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
    // Server is leaked intentionally — it lives for the duration of the test
    // process so the spawned accept loop keeps running.
    std::mem::forget(server);
    addr.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_and_recv_across_nodes() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new(None, "node-b".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("node-b", "/remote/echo"),
    }
    .register(
        &mut node_b,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect("register remote actor");

    let mut node_a = ActorSystem::new(None, "node-a".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    let result = node_a
        .send_and_recv::<RemoteActor>(
            Address::new("node-b", "/remote/echo"),
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

    let mut node_b = ActorSystem::new(None, "node-b2".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("node-b2", "/remote/sink"),
    }
    .register(
        &mut node_b,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect("register remote actor");

    let mut node_a = ActorSystem::new(None, "node-a2".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    node_a
        .send::<RemoteActor>(Address::new("node-b2", "/remote/sink"), RemoteMsg::Bye)
        .await
        .expect("fire across nodes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_routing_when_address_node_matches_self() {
    let broker = spawn_broker().await;

    let mut system = ActorSystem::new(None, "solo".into(), Some(broker.clone()))
        .await
        .expect("system connect");

    RemoteActor {
        addr: Address::new("solo", "/local/only"),
    }
    .register(
        &mut system,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect("register local actor");

    // Address.node == self.node_name → falls through to the local mailbox path,
    // no broker round trip, no `Codec` traffic.
    let result = system
        .send_and_recv::<RemoteActor>(
            Address::new("solo", "/local/only"),
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

    let mut node_a = ActorSystem::new(None, "node-a4".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    // Address.node = "node-b4" but registering on node_a4 → reject.
    let err = RemoteActor {
        addr: Address::new("node-b4", "/foreign/echo"),
    }
    .register(
        &mut node_a,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect_err("foreign-node address should be rejected");

    match err {
        ActorError::AddressNotOwned(s) => assert_eq!(s, "node-b4:/foreign/echo"),
        other => panic!("expected AddressNotOwned, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_tx_cache_variants_route_remotely() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new(None, "node-b5".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("node-b5", "/remote/nocache"),
    }
    .register(
        &mut node_b,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect("register remote actor");

    let node_a = ActorSystem::new(None, "node-a5".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");

    node_a
        .send_without_tx_cache::<RemoteActor>(
            Address::new("node-b5", "/remote/nocache"),
            RemoteMsg::Bye,
        )
        .await
        .expect("send_without_tx_cache across nodes");

    let result = node_a
        .send_and_recv_without_tx_cache::<RemoteActor>(
            Address::new("node-b5", "/remote/nocache"),
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

    let mut node_b = ActorSystem::new(None, "node-b6".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("node-b6", "/bc/remote/1"),
    }
    .register(
        &mut node_b,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect("register remote 1");
    RemoteActor {
        addr: Address::new("node-b6", "/bc/remote/2"),
    }
    .register(
        &mut node_b,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect("register remote 2");

    let mut node_a = ActorSystem::new(None, "node-a6".into(), Some(broker.clone()))
        .await
        .expect("node-a connect");
    // Local actor on node-a that also matches the regex.
    RemoteActor {
        addr: Address::new("node-a6", "/bc/local/1"),
    }
    .register(
        &mut node_a,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect("register local");

    // Peers fan-out includes both self and remote node.
    let results = node_a
        .send_broadcast::<RemoteActor>(
            "/bc/*".into(),
            NodeFilter::Peers(vec!["node-a6".into(), "node-b6".into()]),
            RemoteMsg::Bye,
        )
        .await;

    // local: 1 match on node-a6 (/bc/local/1). remote: 1 BroadcastFire envelope to node-b6.
    assert_eq!(results.local.len(), 1, "expected 1 local match, got {:?}", results.local);
    assert_eq!(results.remote.len(), 1, "expected 1 remote peer ack, got {:?}", results.remote);
    assert!(results.all_ok(), "fan-out failed: {:?}", results);
}

/// Mirror of `test_bounded::test_with_tx_cache` adapted for the multi-node
/// `Address` API. Verifies that the local-routing fast path (when
/// `address.node == self.node_name`) still exercises register / duplicate
/// rejection / broadcast / send_and_recv / restart / run_job lifecycle /
/// unregister under the `multi-node` feature.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn basic_behavior_with_tx_cache() {
    let broker = spawn_broker().await;

    // A second node so the broadcast exercises actual cross-node fan-out.
    let mut peer = ActorSystem::new(None, "basic-c-peer".into(), Some(broker.clone()))
        .await
        .expect("peer system");
    RemoteActor {
        addr: Address::new("basic-c-peer", "/a/3"),
    }
    .register(&mut peer, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("/a/3 on peer");

    let mut sys = ActorSystem::new(None, "basic-c".into(), Some(broker.clone()))
        .await
        .expect("system");

    // Register a few local actors.
    RemoteActor {
        addr: Address::new("basic-c", "/a/1"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("/a/1");
    RemoteActor {
        addr: Address::new("basic-c", "/a/2"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("/a/2");
    RemoteActor {
        addr: Address::new("basic-c", "/b/1"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("/b/1");

    // Duplicate same-node, same-name registration → AddressAlreadyExist.
    let dup_err = RemoteActor {
        addr: Address::new("basic-c", "/a/1"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect_err("duplicate address must be rejected");
    assert!(matches!(dup_err, ActorError::AddressAlreadyExist(_)));

    // Broadcast across both nodes: 2 local matches (/a/1, /a/2 on basic-c) +
    // 1 BroadcastFire envelope to basic-c-peer (handled there against /a/3).
    let bc = sys
        .send_broadcast::<RemoteActor>(
            "/a/*".into(),
            NodeFilter::Peers(vec!["basic-c".into(), "basic-c-peer".into()]),
            RemoteMsg::Bye,
        )
        .await;
    assert_eq!(bc.local.len(), 2, "expected 2 local matches, got {:?}", bc.local);
    assert_eq!(bc.remote.len(), 1, "expected 1 remote peer ack, got {:?}", bc.remote);
    assert!(bc.all_ok(), "broadcast failed: {:?}", bc);

    // send_and_recv against a local actor.
    let resp = sys
        .send_and_recv::<RemoteActor>(
            Address::new("basic-c", "/b/1"),
            RemoteMsg::Ping("hello".into()),
        )
        .await
        .expect("send_and_recv");
    assert!(matches!(resp, RemoteMsg::Echo(ref s) if s == "pong:hello"));

    // Restart the /a/* actors and verify they recover.
    sys.restart("/a/*".into());
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let resp = sys
        .send_and_recv::<RemoteActor>(
            Address::new("basic-c", "/a/1"),
            RemoteMsg::Ping("after-restart".into()),
        )
        .await
        .expect("send_and_recv after restart");
    assert!(matches!(resp, RemoteMsg::Echo(ref s) if s == "pong:after-restart"));

    // Same call shape via a cloned system (caches are per-clone).
    let mut sys_clone = sys.clone();
    let resp = sys_clone
        .send_and_recv::<RemoteActor>(
            Address::new("basic-c", "/b/1"),
            RemoteMsg::Ping("from-clone".into()),
        )
        .await
        .expect("send_and_recv from clone");
    assert!(matches!(resp, RemoteMsg::Echo(ref s) if s == "pong:from-clone"));

    // run_job with a bounded iteration count.
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
            Address::new("basic-c", "/a/1"),
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
    assert_eq!(got, 2, "expected exactly 2 iterations");

    // run_job with infinite interval + stop/resume/abort controls.
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
            Address::new("basic-c", "/a/1"),
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

    // Unregister everything; subsequent sends should fail.
    sys.unregister("*".into());
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let err = sys
        .send_and_recv::<RemoteActor>(
            Address::new("basic-c", "/b/1"),
            RemoteMsg::Ping("after-unregister".into()),
        )
        .await
        .expect_err("send after unregister must fail");
    let _ = err;
}

/// Mirror of `test_bounded::test_without_tx_cache`: same behavior but via the
/// non-cached `_without_tx_cache` variants. Verifies their multi-node code
/// paths still handle the local case (`address.node == self.node_name`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn basic_behavior_without_tx_cache() {
    let broker = spawn_broker().await;

    let mut peer = ActorSystem::new(None, "basic-nc-peer".into(), Some(broker.clone()))
        .await
        .expect("peer system");
    RemoteActor {
        addr: Address::new("basic-nc-peer", "/a/9"),
    }
    .register(&mut peer, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("/a/9 on peer");

    let mut sys = ActorSystem::new(None, "basic-nc".into(), Some(broker.clone()))
        .await
        .expect("system");

    RemoteActor {
        addr: Address::new("basic-nc", "/a/1"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("/a/1");
    RemoteActor {
        addr: Address::new("basic-nc", "/a/2"),
    }
    .register(&mut sys, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("/a/2");

    // Non-cached broadcast across both nodes.
    let bc = sys
        .send_broadcast_without_tx_cache::<RemoteActor>(
            "/a/*".into(),
            NodeFilter::Peers(vec!["basic-nc".into(), "basic-nc-peer".into()]),
            RemoteMsg::Bye,
        )
        .await;
    assert_eq!(bc.local.len(), 2, "expected 2 local matches, got {:?}", bc.local);
    assert_eq!(bc.remote.len(), 1, "expected 1 remote peer ack, got {:?}", bc.remote);
    assert!(bc.all_ok());

    // send_and_recv non-cached.
    let resp = sys
        .send_and_recv_without_tx_cache::<RemoteActor>(
            Address::new("basic-nc", "/a/2"),
            RemoteMsg::Ping("nc".into()),
        )
        .await
        .expect("send_and_recv_without_tx_cache");
    assert!(matches!(resp, RemoteMsg::Echo(ref s) if s == "pong:nc"));

    // send fire non-cached.
    sys.send_without_tx_cache::<RemoteActor>(
        Address::new("basic-nc", "/a/1"),
        RemoteMsg::Bye,
    )
    .await
    .expect("send_without_tx_cache");

    // run_job_without_tx_cache with max_iter.
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
            Address::new("basic-nc", "/a/1"),
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
/// Mirrors `test_bounded::test_bench_with_tx_cache` but every iteration round
/// trips through the broker (encode → produce → consume → decode → handle →
/// encode reply → produce → consume → decode).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bench_send_and_recv_across_nodes_with_tx_cache() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new(None, "bench-b".into(), Some(broker.clone()))
        .await
        .expect("node-b");
    RemoteActor {
        addr: Address::new("bench-b", "/bench/echo"),
    }
    .register(&mut node_b, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("register bench actor");

    let mut node_a = ActorSystem::new(None, "bench-a".into(), Some(broker.clone()))
        .await
        .expect("node-a");

    let payload = "x".repeat(1024); // 1 KiB
    let target = Address::new("bench-b", "/bench/echo");
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
        "[bench] cross-node send_and_recv with_tx_cache: {} iters, {} ms ({:.2} ms/op)",
        ITERS,
        elapsed.as_millis(),
        elapsed.as_secs_f64() * 1000.0 / ITERS as f64
    );
}

/// Same bench but via `send_and_recv_without_tx_cache` — every iteration also
/// re-does the `FindActor` round trip locally before the cross-node hop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bench_send_and_recv_across_nodes_without_tx_cache() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new(None, "bench-b-nc".into(), Some(broker.clone()))
        .await
        .expect("node-b");
    RemoteActor {
        addr: Address::new("bench-b-nc", "/bench/echo"),
    }
    .register(&mut node_b, ErrorHandling::Stop, Blocking::NonBlocking, None)
    .await
    .expect("register bench actor");

    let node_a = ActorSystem::new(None, "bench-a-nc".into(), Some(broker.clone()))
        .await
        .expect("node-a");

    let payload = "x".repeat(1024);
    let target = Address::new("bench-b-nc", "/bench/echo");
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
        "[bench] cross-node send_and_recv without_tx_cache: {} iters, {} ms ({:.2} ms/op)",
        ITERS,
        elapsed.as_millis(),
        elapsed.as_secs_f64() * 1000.0 / ITERS as f64
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_job_against_remote_actor() {
    let broker = spawn_broker().await;

    let mut node_b = ActorSystem::new(None, "node-b7".into(), Some(broker.clone()))
        .await
        .expect("node-b connect");
    RemoteActor {
        addr: Address::new("node-b7", "/remote/job"),
    }
    .register(
        &mut node_b,
        ErrorHandling::Resume,
        Blocking::NonBlocking,
        None,
    )
    .await
    .expect("register remote job actor");

    let mut node_a = ActorSystem::new(None, "node-a7".into(), Some(broker.clone()))
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
            Address::new("node-b7", "/remote/job"),
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
