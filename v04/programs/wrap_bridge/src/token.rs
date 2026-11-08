//! The canonical token program on the actor model (logos-blockchain/lez-programs,
//! branch `refactor/lez-v0.4.x` at 2801ea1, `programs/token/core/src/lib.rs`),
//! mirrored: its crate pins another LEZ revision, and one binary cannot hold
//! two `lee_core`s. The borsh layouts are pinned against the real program by
//! the executor tests (v04/executor-tests), which run its guest.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::AccountId;

/// A token actor's state: a definition or a holding.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum TokenState {
    Definition(TokenDefinition),
    Holding(TokenHolding),
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TokenDefinition {
    pub name: String,
    pub total_supply: u128,
    pub authority: Option<AccountId>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TokenHolding {
    pub definition_id: AccountId,
    pub balance: u128,
}

/// `token_core::Message`, the messages anyone may send.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// To the definition (authorized): writes it and initializes `holding`
    /// with the supply (`holding` must be authorized too, down the sync chain).
    NewDefinition { name: String, total_supply: u128, mint_authority: Option<AccountId>, holding: AccountId },
    /// To the holding (authorized): holding -> definition -> holding.
    InitializeHolding { definition: AccountId },
    /// To the sending holding (authorized): debit, then an async credit of
    /// `to`, which refuses another definition's tokens.
    Transfer { to: AccountId, amount: u128 },
    /// To the holding (authorized): debit, then the definition's supply.
    Burn { amount: u128 },
    /// To the authority's actor (authorized), which forwards to the definition.
    Mint { definition: AccountId, to: AccountId, amount: u128 },
    SetAuthority { definition: AccountId, new_authority: Option<AccountId> },
    ReadState,
}
