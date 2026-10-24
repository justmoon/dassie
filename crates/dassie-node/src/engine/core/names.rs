//! The owner's names (spec/areas/network.md#the-registrant): registering a
//! name for the node at every registrar, renewing it, and how each stands.
//! The peer node does it all; here are the owner's requests and the view.

use dassie_identity::NodeId;
use dassie_peer::names::registrant::{RegisterError, RenewError};
use dassie_peer::names::{Name, NameStatus};

use super::{Core, EngineError};

/// The owner's names, as the node sees them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamesView {
    /// Our node ID.
    pub own: NodeId,
    /// The registrars: the configured bootstrap nodes.
    pub registrars: Vec<NodeId>,
    /// Whether the node's view of who holds each name is frozen: too few
    /// of the registrars' lists are fresh.
    pub frozen: bool,
    /// The names the node registers or holds.
    pub names: Vec<NameStatus>,
}

impl Core {
    /// How the owner's names stand.
    pub fn names(&self) -> NamesView {
        let holders = self.peer.holders();
        let mut registrars: Vec<NodeId> = self
            .peer
            .registration_status()
            .iter()
            .map(|status| status.bootstrap)
            .collect();
        registrars.sort_unstable();
        NamesView {
            own: *self.peer.own(),
            registrars,
            frozen: holders.registrars() > 0 && holders.is_frozen(),
            names: self.peer.name_statuses(self.now_ms),
        }
    }

    /// Registers `name` for the node at every registrar: committed sealed,
    /// and unsealed once every registrar has logged it.
    ///
    /// # Errors
    ///
    /// The name is not one an owner may register, the node holds it or
    /// another does, it has no registrars, too many registrations are
    /// under way, or it holds as many names as a registrar grants it.
    pub fn register_name(&mut self, name: &str) -> Result<(), EngineError> {
        let name =
            Name::registrable(name).map_err(|error| EngineError::Invalid(error.to_string()))?;
        let now = self.now_ms;
        self.peer.register_name(now, name).map_err(register_error)?;
        self.changes.network = true;
        self.after_peer();
        Ok(())
    }

    /// Renews `name`, which the node holds, at every registrar now.
    ///
    /// # Errors
    ///
    /// The node does not hold the name.
    pub fn renew_name(&mut self, name: &str) -> Result<(), EngineError> {
        let name = Name::parse(name).map_err(|error| EngineError::Invalid(error.to_string()))?;
        let now = self.now_ms;
        self.peer
            .renew_name(now, &name)
            .map_err(|RenewError::NotHeld| {
                EngineError::NotFound("the node does not hold that name".into())
            })?;
        self.changes.network = true;
        self.after_peer();
        Ok(())
    }
}

fn register_error(error: RegisterError) -> EngineError {
    match error {
        RegisterError::Reserved => EngineError::Invalid("the name is reserved".into()),
        RegisterError::NoRegistrars => EngineError::Precondition(
            "the node has no bootstrap nodes to register a name with".into(),
        ),
        RegisterError::Held => EngineError::Precondition("the node holds that name".into()),
        RegisterError::Taken => EngineError::Precondition("the name is taken".into()),
        RegisterError::UnderWay => {
            EngineError::Precondition("a registration of that name is under way".into())
        }
        RegisterError::TooMany => {
            EngineError::Exhausted("too many registrations are under way".into())
        }
        RegisterError::Names => {
            EngineError::Exhausted("the node holds as many names as a registrar grants it".into())
        }
        RegisterError::Full => {
            EngineError::Exhausted("too many registrations, none of them finished".into())
        }
    }
}
