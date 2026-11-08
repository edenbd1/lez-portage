//! The Attest half of Sextant on LEZ v0.4.0-rc1.
//!
//! Sextant (lez-sextant `programs/sextant`, LEZ v0.3) keeps one immutable
//! header record per finalized Ethereum header, at
//! `sha256("header" ‖ instance_id ‖ slot)`, and its `Attest` refuses unless a
//! query (a storage or receipt proof and predicates) holds against that
//! record. On the actor model the record is the state of this program's actor
//! on that account, and `Attest` is a message to it: a refusal fails the
//! sender's whole transaction, which is all a consumer needs. The check is
//! sextant-core's `statement::evaluate`, unchanged.
//!
//! What is NOT here: the light client (`Init`, `Begin`/`Miller`/`Finish`),
//! which verifies sync-committee signatures and writes the records. In this
//! spike a record is written by `Record`, sent to the actor of one writer key,
//! which must sign; the instance id is `sha256("sextant-attest/writer" ‖
//! writer)`, so a consumer pinning an instance id pins that one key. It is a
//! federation of one standing in for the unported light client, and it is
//! stated as such wherever it is used.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use sextant_core::statement::{self, HeaderRecord, Query};

pub type H256 = [u8; 32];

pub const E_ACCOUNTS: u32 = 7290;
pub const E_EXISTS: u32 = 7291;
pub const E_NO_RECORD: u32 = 7292;
pub const E_ATTEST: u32 = 7296;
pub const E_NOT_WRITER: u32 = 7297;

#[track_caller]
fn refuse(code: u32, why: &str) -> ! {
    panic!("E{code}: {why}")
}

#[track_caller]
fn refuse_unless(ok: bool, code: u32, why: &str) {
    if !ok {
        refuse(code, why);
    }
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// To the header record actor: refuses unless `query` holds against it.
    Attest { instance_id: H256, slot: u64, query: Query },
    /// To the writer's own actor (authorized): forwards the record.
    Record { slot: u64, record: HeaderRecord },
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum Internal {
    Write { record: HeaderRecord },
}

pub fn header_seed(instance_id: &H256, slot: u64) -> PdaSeed {
    PdaSeed::new(wrap_core::sha256(&[b"header", instance_id, &slot.to_le_bytes()]))
}

pub fn header_account(me: &AccountId, instance_id: &H256, slot: u64) -> AccountId {
    AccountId::for_public_pda(me, &header_seed(instance_id, slot))
}

/// The instance a writer key's records live under.
pub fn writer_instance(writer: &AccountId) -> H256 {
    wrap_core::sha256(&[b"sextant-attest/writer", writer.value()])
}

pub fn handle_message(input: &ReceiveInput) -> Response {
    let me = input.receiver.program_account_id;
    if input.from_own_program() {
        let Internal::Write { record } = borsh::from_slice(&input.message).expect("an internal message");
        let new = borsh::to_vec(&record).expect("infallible");
        refuse_unless(input.pre_state.is_empty() || *input.pre_state == *new, E_EXISTS, "a different record exists for this slot");
        return Response::set_state(new);
    }
    match borsh::from_slice::<Message>(&input.message).unwrap_or_else(|_| refuse(E_ACCOUNTS, "not a sextant_attest message")) {
        Message::Attest { instance_id, slot, query } => {
            refuse_unless(input.receiver.account_id == header_account(&me, &instance_id, slot), E_ACCOUNTS, "Attest goes to the header PDA of this instance and slot");
            refuse_unless(!input.pre_state.is_empty(), E_NO_RECORD, "no header record at this slot");
            let r: HeaderRecord = borsh::from_slice(&input.pre_state).expect("a header record");
            refuse_unless(r.slot == slot, E_ACCOUNTS, "the record is another slot's");
            if let Err(e) = statement::evaluate(&r, &query) {
                refuse(E_ATTEST, &format!("the query does not hold: {e:?}"));
            }
            Response::keep_state()
        }
        Message::Record { slot, record } => {
            refuse_unless(input.is_authorized, E_NOT_WRITER, "the writer must sign");
            refuse_unless(record.slot == slot, E_ACCOUNTS, "the record is another slot's");
            let instance = writer_instance(&input.receiver.account_id);
            let header = Actor::new(header_account(&me, &instance, slot), me);
            Response::keep_state().send(SyncMessage::new(header, &Internal::Write { record }).with_pda_seeds([header_seed(&instance, slot)]))
        }
    }
}
