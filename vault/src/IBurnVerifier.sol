// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

/// @notice What the vault asks before it releases: is `proof` a valid proof that a burn with
/// these public inputs is final on the paired LEZ deployment?
///
/// The vault fixes its verifier at construction and never changes it (RFP-022 Security #8). A
/// production verifier is a RISC Zero Groth16 verifier pinned to one release-guest image id; the
/// guest checks LEZ finality for the burn tree root and the burn's membership under it, without
/// revealing which burn it is. `publicInputs` is `WrapVault.releaseDigest(...)`.
interface IBurnVerifier {
    function verify(bytes calldata proof, bytes32 publicInputs) external view returns (bool);
}
