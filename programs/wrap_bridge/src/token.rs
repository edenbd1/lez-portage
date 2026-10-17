//! The instructions of LEZ v0.3.0's token program (`lez/programs/token/core`,
//! `db66590ab`) this program sends, mirrored so the guest does not link that
//! crate (edition 2024, serde, the LEZ workspace). Variant order is the wire
//! format; `executor-tests/tests/bridge.rs` checks these bytes against
//! `token_core::Instruction` itself.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::AccountId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum TokenKind {
    Fungible,
    NftMaster,
    NftPrintedCopy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TokenDescriptor {
    pub definition_id: AccountId,
    pub kind: TokenKind,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// 0. Unused here.
    Transfer { amount_to_transfer: u128, descriptor: TokenDescriptor },
    /// 1. Rows: [definition (empty), holding (empty)].
    NewFungibleDefinition { name: String, total_supply: u128 },
    /// 2. Unused here (its payload is not mirrored).
    NewDefinitionWithMetadata,
    /// 3. Unused here.
    InitializeAccount { kind: TokenKind },
    /// 4. Rows: [definition, holding (authorized)].
    Burn { amount_to_burn: u128, kind: TokenKind },
    /// 5. Rows: [definition (authorized), holding].
    Mint { amount_to_mint: u128 },
}

/// `token_core::TokenHolding`, to read balances.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum TokenHolding {
    Fungible { definition_id: AccountId, balance: u128 },
    NftMaster { definition_id: AccountId, print_balance: u128 },
    NftPrintedCopy { definition_id: AccountId, owned: bool },
}

/// `token_core::TokenDefinition`, to read supply.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum TokenDefinition {
    Fungible { name: String, total_supply: u128, metadata_id: Option<AccountId> },
    NonFungible { name: String, printable_supply: u128, metadata_id: AccountId },
}

/// `associated_token_account_core::AtaContents`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum AtaContents {
    Empty,
    Intended,
    Squatted,
}

/// The instructions of LEZ v0.3.0's associated-token-account program
/// (`lez/programs/associated_token_account/core`), mirrored the same way.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum AtaInstruction {
    /// 0. Rows: [owner, definition, ata].
    Create { token_program_id: AccountId, kind: TokenKind, contents: AtaContents },
    /// 1. Rows: [owner (authorized), sender ata, recipient holding].
    Transfer { token_program_id: AccountId, descriptor: TokenDescriptor, amount: u128 },
    /// 2. Rows: [owner (authorized), ata, definition].
    Burn { token_program_id: AccountId, kind: TokenKind, amount: u128 },
}

/// `associated_token_account_core::compute_ata_seed` and
/// `get_associated_token_account_id`: the holding of `definition` that
/// `owner` controls, in `token_program`'s shard.
pub fn ata_account(ata_program: &AccountId, owner: &AccountId, definition: &AccountId, token_program: &AccountId) -> AccountId {
    let seed = lee_core::program::PdaSeed::new(wrap_core::sha256(&[owner.value(), definition.value(), token_program.value()]));
    AccountId::for_public_pda(ata_program, &seed)
}
