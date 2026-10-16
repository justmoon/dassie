//! Places for connections, shared fairly between clients.
//!
//! A [`ConnectionTable`] behind a mutex, handing out guards: a place is
//! taken when a connection arrives and returned when its guard drops, which
//! is when the connection's task ends, however it ends.

use std::sync::{Arc, Mutex};

use dassie_node_core::connections::{ConnectionLimits, ConnectionTable, Refused};
use dassie_node_core::rate_limit::ClientKey;

use crate::node::lock;

/// A pool of places for connections, with a share per client.
#[derive(Clone, Debug)]
pub struct ClientSlots(Arc<Mutex<ConnectionTable>>);

impl ClientSlots {
    /// An empty pool.
    #[must_use]
    pub fn new(limits: ConnectionLimits) -> Self {
        Self(Arc::new(Mutex::new(ConnectionTable::new(limits))))
    }

    /// Takes a place for a connection from `client`; `None` counts in the
    /// total only.
    ///
    /// # Errors
    ///
    /// The pool is full, or the client has its share.
    pub fn take(&self, client: Option<ClientKey>) -> Result<ClientSlot, Refused> {
        lock(&self.0).open(client)?;
        Ok(ClientSlot {
            slots: self.clone(),
            client,
        })
    }
}

/// One connection's place, returned when it drops.
#[derive(Debug)]
pub struct ClientSlot {
    slots: ClientSlots,
    client: Option<ClientKey>,
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        lock(&self.slots.0).close(self.client);
    }
}
