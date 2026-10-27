//! What two members exchange to peer over their hub: the hub's ID, and
//! each one's account number and account key. A node refuses a peer in
//! another hub. The key is the one the peer's promises are checked with;
//! the payee takes them only once an operation the hub executed on the
//! hold verified under it, so a peer that gave another key than its
//! account's gains nothing by it.
//!
//! A member the hub has no account for yet presents its key and account
//! [`NONE`]: the hub counts its accounts from 1, so it never names one. A
//! peer with an account pays it into the hub, with `pay` to that key, once
//! it owes it the hub's minimum for a new account
//! (spec/designs/settlement-hubs.md#5-joining). Two members of which
//! neither has an account do not peer: neither could pay the other.

use alloc::vec::Vec;

use dassie_identity::PublicKey;
use dassie_oer::{Encode, Reader, Writer};
use dassie_settlement::SchemeError;

use crate::codec::{read_key, write_key};
use crate::digest::Digest;
use crate::operation::AccountNumber;
use crate::rules::HubId;

/// The account of a member the hub has none for yet.
pub(super) const NONE: AccountNumber = AccountNumber(dassie_settlement::Membership::NO_ACCOUNT);

/// A member as it presents itself to a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Membership {
    pub(super) hub: HubId,
    /// Its account, or [`NONE`].
    pub(super) account: AccountNumber,
    pub(super) key: PublicKey,
}

const MEMBERSHIP_LEN: usize = 32 + 4 + 32;

impl Encode for Membership {
    fn encoded_len(&self) -> usize {
        MEMBERSHIP_LEN
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(self.hub.0.as_bytes());
        writer.write_u32(self.account.0);
        write_key(writer, &self.key);
    }
}

impl Membership {
    /// Writes it to `out`, replacing what was there.
    pub(super) fn write(&self, out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(&self.to_vec());
    }

    /// Reads one that fills `bytes`, of a member of `hub`.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidData`] for one that is not, or of another hub.
    pub(super) fn read(bytes: &[u8], hub: &HubId) -> Result<Self, SchemeError> {
        let mut reader = Reader::new(bytes);
        let parsed = (|| {
            let id = HubId(Digest(*reader.read_array::<32>().ok()?));
            let account = AccountNumber(reader.read_u32().ok()?);
            let key = read_key(&mut reader).ok()?;
            reader.finish().ok()?;
            Some(Self {
                hub: id,
                account,
                key,
            })
        })()
        .ok_or(SchemeError::InvalidData)?;
        if parsed.hub != *hub {
            return Err(SchemeError::InvalidData);
        }
        Ok(parsed)
    }
}

/// The account in `hub` of the peer whose state, as the scheme keeps it
/// for a peering, is `state`: how the node tells which of its holds is
/// toward which peer. A peer that had no account when it peered gave
/// account 0, the hub's never; the scheme learns its account once it has
/// one ([`HubScheme::peer_account`](super::HubScheme::peer_account)).
#[must_use]
pub fn peer_account(state: &[u8], hub: &HubId) -> Option<AccountNumber> {
    Membership::read(state, hub).ok().map(|m| m.account)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dassie_identity::NodeKey;

    #[test]
    fn a_member_of_another_hub_is_refused() {
        let hub = HubId(Digest([1; 32]));
        let member = Membership {
            hub,
            account: AccountNumber(9),
            key: NodeKey::from_bytes(&[3; 32]).public_key(),
        };
        let mut bytes = Vec::new();
        member.write(&mut bytes);
        assert_eq!(Membership::read(&bytes, &hub), Ok(member));
        assert_eq!(
            Membership::read(&bytes, &HubId(Digest([2; 32]))),
            Err(SchemeError::InvalidData)
        );
        assert!(Membership::read(&bytes[1..], &hub).is_err());
        bytes.push(0);
        assert!(Membership::read(&bytes, &hub).is_err());
    }

    #[test]
    fn a_member_without_an_account_presents_its_key() {
        let hub = HubId(Digest([1; 32]));
        let member = Membership {
            hub,
            account: NONE,
            key: NodeKey::from_bytes(&[3; 32]).public_key(),
        };
        let mut bytes = Vec::new();
        member.write(&mut bytes);
        assert_eq!(bytes.len(), MEMBERSHIP_LEN, "the same length as any");
        assert_eq!(Membership::read(&bytes, &hub), Ok(member));
        assert_eq!(peer_account(&bytes, &hub), Some(NONE));
    }
}
