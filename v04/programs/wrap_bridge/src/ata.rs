//! The associated token account program on the actor model
//! (logos-blockchain/lez-programs, branch `refactor/ata-lez-v0.4.x` at
//! 064b73f, `programs/ata/core`), mirrored for the same reason as
//! [`crate::token`]. The borsh layout and the address derivation are pinned
//! against the real program by the executor tests, which run its guest.
//!
//! An owner's holding of a definition is the token actor of a PDA of the ATA
//! program, `sha256(token_program ‖ owner ‖ definition)`: one per owner and
//! definition, so one account holds any number of wrapped tokens.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{account::AccountId, program::PdaSeed};

/// `ata_core::Message`: the first three variants (the NFT ones follow them).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// To any ATA actor, permissionless and idempotent: initialize `owner`'s
    /// holding of `definition` (declare the holding's and the definition's
    /// token actors).
    Create { token_program_id: AccountId, owner: AccountId, definition: AccountId },
    /// To the owner's ATA actor (authorized).
    Transfer { token_program_id: AccountId, definition: AccountId, to: AccountId, amount: u128 },
    /// To the owner's ATA actor (authorized): burn from the owner's holding.
    Burn { token_program_id: AccountId, definition: AccountId, amount: u128 },
}

/// `ata_core::compute_ata_seed`.
#[must_use]
pub fn seed(token_program: &AccountId, owner: &AccountId, definition: &AccountId) -> PdaSeed {
    PdaSeed::new(wrap_core::sha256(&[token_program.value(), owner.value(), definition.value()]))
}

/// `ata_core::get_associated_token_account_id`: the account whose token actor
/// is `owner`'s holding of `definition`.
#[must_use]
pub fn holding(ata_program: &AccountId, token_program: &AccountId, owner: &AccountId, definition: &AccountId) -> AccountId {
    AccountId::for_public_pda(ata_program, &seed(token_program, owner, definition))
}
