// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {EIP712} from "openzeppelin/utils/cryptography/EIP712.sol";
import {ECDSA} from "openzeppelin/utils/cryptography/ECDSA.sol";

/// @notice Records that a wallet belongs to a verified human.
///
/// World ID proofs are checked off-chain against the Developer Portal, because
/// the World ID router lives on World Chain rather than Arc. The backend signs
/// the result, but the relayer submits it and pays the gas — not the wallet
/// being attested for. `wallet` here is not a user's own wallet either: it is
/// a pseudonymous address the backend derives from the nullifier hash, so one
/// person maps to one record with nobody holding a key to it. It is a name,
/// not a wallet.
///
/// A Selfie Check credential is good for 90 days, which is where `expiresAt`
/// comes from. The free lane lapses with the credential.
contract HumanRegistry is EIP712 {
    bytes32 private constant ATTESTATION_TYPEHASH =
        keccak256("Attestation(address wallet,bytes32 nullifierHash,uint40 expiresAt)");

    address public immutable attester;

    mapping(address wallet => uint40 expiresAt) public humanUntil;

    /// @notice One World ID nullifier can only ever back one wallet. In
    /// practice `wallet` is already derived from `nullifierHash` off-chain, so
    /// the binding is one-to-one before this check ever runs — this mapping is
    /// defence-in-depth against a future caller that does not derive it that
    /// way, not the thing that stops a person minting unlimited free senders
    /// today.
    mapping(bytes32 nullifierHash => address wallet) public nullifierOwner;

    /// @notice A wallet's personhood was attested, valid until `expiresAt`.
    /// The subgraph's sole source for who is currently verified.
    event HumanAttested(address indexed wallet, bytes32 indexed nullifierHash, uint40 expiresAt);

    error InvalidSignature();
    error AttestationExpired();
    error NullifierAlreadyBound(address boundTo);
    error NotAnExtension(uint40 current);

    // This is a separate EIP-712 domain from PostageEscrow's, versioned "1"
    // here against its "2" — see the constructor there for why they differ.
    // Off-chain signers hardcode this string; changing it breaks every
    // attestation signature silently.
    constructor(address attester_) EIP712("Postage", "1") {
        // Reusing InvalidSignature for a bad constructor argument is a wart:
        // no signature was involved. Kept as-is because renaming or adding an
        // error here changes this contract's deployed interface.
        if (attester_ == address(0)) revert InvalidSignature();
        attester = attester_;
    }

    /// @notice Records that `wallet` attested as human until `expiresAt`,
    /// given a signature from `attester` over the claim. Called by the
    /// relayer, not by `wallet` itself — see the contract header.
    function attest(
        address wallet,
        bytes32 nullifierHash,
        uint40 expiresAt,
        bytes calldata signature
    ) external {
        if (expiresAt <= block.timestamp) revert AttestationExpired();

        address boundTo = nullifierOwner[nullifierHash];
        if (boundTo != address(0) && boundTo != wallet) revert NullifierAlreadyBound(boundTo);

        // Refusing anything that is not an extension stops an old signature
        // being replayed to shorten a renewed attestation.
        uint40 current = humanUntil[wallet];
        if (expiresAt <= current) revert NotAnExtension(current);

        bytes32 digest = _hashTypedDataV4(
            keccak256(abi.encode(ATTESTATION_TYPEHASH, wallet, nullifierHash, expiresAt))
        );
        if (ECDSA.recover(digest, signature) != attester) revert InvalidSignature();

        nullifierOwner[nullifierHash] = wallet;
        humanUntil[wallet] = expiresAt;

        emit HumanAttested(wallet, nullifierHash, expiresAt);
    }

    /// @notice Whether `wallet`'s attestation is still current. Unused by any
    /// on-chain caller, web, worker, or subgraph as of this writing — callers
    /// read `humanUntil` directly instead. Kept rather than removed: removing
    /// a public function changes this contract's deployed interface for no
    /// benefit, since nothing is saved by it going away.
    function isHuman(address wallet) external view returns (bool) {
        return humanUntil[wallet] > block.timestamp;
    }

    /// @notice Exposes the EIP-712 domain separator this contract signs
    /// against. Unused by any on-chain caller, web, worker, or subgraph as of
    /// this writing — off-chain code reconstructs the domain from `name` and
    /// `version` rather than reading it here. Kept rather than removed for the
    /// same reason as `isHuman`.
    function domainSeparator() external view returns (bytes32) {
        return _domainSeparatorV4();
    }
}
