//! A registrant of names on the simulated network: a key with sessions of
//! its own that sends requests to the registrars, as a registrant node
//! would (spec/designs/names.md#3-registering).

#![allow(dead_code)]

use std::net::IpAddr;
use std::sync::Arc;

use dassie_identity::{NodeId, NodeKey};
use dassie_kit::sim::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_peer::Reply;
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::names::{Commitment, Name, Salt};
use dassie_peer::sim::{Network, START, key};
use dassie_peer::wire::{Authentication, Content, NameReceipt, ResponseEnvelope, encode_envelope};

/// A registrant: a key with sessions of its own.
pub struct Registrant {
    auth: Authenticator,
    rng: SeededRng,
}

impl Registrant {
    pub fn new(seed: u8) -> Self {
        let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
        Self {
            auth: Authenticator::new(key, AuthConfig::DEFAULT, START - 1),
            rng: SeededRng::new(u64::from(seed)),
        }
    }

    pub fn id(&self) -> NodeId {
        self.auth.node_key().node_id()
    }

    pub fn salt(&self) -> Salt {
        Salt([self.id().as_str().as_bytes()[0]; 32])
    }

    pub fn commitment(&self, name: &Name) -> Commitment {
        Commitment::seal(name, &self.id(), &self.salt())
    }

    /// Sends `content` to node `to`, authenticated, from `source`.
    pub fn send(
        &mut self,
        network: &mut Network,
        to: usize,
        content: &Content<'_>,
        source: Option<IpAddr>,
    ) -> Reply {
        let bytes = content.to_vec();
        let target = key(to);
        let (hmac, _) = self
            .auth
            .authenticate(
                network.clock(to),
                &mut self.rng,
                &target.node_id(),
                &target.public_key(),
                &bytes,
            )
            .unwrap();
        let body = encode_envelope(&self.id(), &Authentication::Hmac(hmac), &bytes);
        network.inbound_from(to, &body, source)
    }

    /// Commits to `name` at registrar `to`.
    pub fn commit_at(&mut self, network: &mut Network, to: usize, name: &Name) -> Reply {
        let content = Content::NameCommit(self.commitment(name));
        self.send(network, to, &content, None)
    }

    /// Unseals `name` at registrar `to`.
    pub fn unseal_at(&mut self, network: &mut Network, to: usize, name: &Name) -> Reply {
        let content = Content::NameUnseal {
            name: *name,
            salt: self.salt(),
        };
        self.send(network, to, &content, None)
    }

    /// Commits to `name` at node 0.
    pub fn commit(&mut self, network: &mut Network, name: &Name) -> Reply {
        self.commit_at(network, 0, name)
    }

    /// Unseals `name` at node 0.
    pub fn unseal(&mut self, network: &mut Network, name: &Name) -> Reply {
        self.unseal_at(network, 0, name)
    }

    /// Renews `name` at node 0.
    pub fn renew(&mut self, network: &mut Network, name: &Name) -> Reply {
        self.send(network, 0, &Content::NameRenew(*name), None)
    }
}

/// The body of a 200.
pub fn body(reply: &Reply) -> Vec<u8> {
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    ResponseEnvelope::decode(&reply.body).unwrap().body.to_vec()
}

/// The receipt in a 200.
pub fn receipt(reply: &Reply) -> NameReceipt {
    NameReceipt::decode_exact(&body(reply)).unwrap()
}

/// `text` as a name.
pub fn name(text: &str) -> Name {
    Name::parse(text).unwrap()
}
