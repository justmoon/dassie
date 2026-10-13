//! Arbitrary frames never make an RPC connection panic, and anything that is
//! not a `ClientMessage` ends it as a protocol error (D16, spec 7 §7).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use dassie_node::clock::ManualClock;
use dassie_node::node::{Node, NodeOptions, Policies, Runtime};
use dassie_node::rpc::connection::{self, Closed};
use dassie_node::rpc::{Caller, proto};
use dassie_node::store::Store;
use dassie_node_core::testing::SeededRng;
use proptest::prelude::*;
use prost::Message;
use tokio_util::sync::CancellationToken;

fn node() -> Arc<Node> {
    Node::new(NodeOptions {
        store: Store::open_in_memory().unwrap(),
        clock: Arc::new(ManualClock::new(1_800_000_000_000)),
        rng: Box::new(SeededRng::new(1)),
        runtime: Runtime::Systemd,
        http_port: 80,
        https_port: 443,
        acme_ca_file: None,
        policies: Policies::DEFAULT,
    })
    .unwrap()
}

fn serve(node: &Arc<Node>, runtime: &tokio::runtime::Runtime, frames: Vec<Vec<u8>>) -> Closed {
    let incoming = futures_util::stream::iter(
        frames
            .into_iter()
            .map(|frame| Ok::<_, Infallible>(Bytes::from(frame))),
    );
    let outgoing = futures_util::sink::drain();
    let caller = Caller::Browser(None);
    runtime.block_on(connection::serve(
        Arc::clone(node),
        caller,
        incoming,
        outgoing,
        CancellationToken::new(),
    ))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_frames(frames in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..8)) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let node = node();
        let malformed = frames.iter().any(|frame| match proto::ClientMessage::decode(frame.as_slice()) {
            Ok(message) => message.kind.is_none(),
            Err(_) => true,
        });
        let closed = serve(&node, &runtime, frames);
        if malformed {
            prop_assert_eq!(closed, Closed::ProtocolError);
        } else {
            prop_assert!(matches!(closed, Closed::ByClient | Closed::ProtocolError));
        }
    }
}
