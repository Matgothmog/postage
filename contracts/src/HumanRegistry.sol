// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {EIP712} from "openzeppelin/utils/cryptography/EIP712.sol";
import {ECDSA} from "openzeppelin/utils/cryptography/ECDSA.sol";

/// @notice Records that a wallet belongs to a verified human.
///
/// World ID proofs are checked off-chain against the Developer Portal, because
/// the World ID router lives on World Chain rather than Arc. The backend signs
/// the result and the user submits it here themselves, so the attestation is
/// self-custodied and the wallet pays its own gas.
///
/// A Selfie Check credential is good for 90 days, which is where `expiresAt`
/// comes from. The free lane lapses with the credential.
contract HumanRegistry is EIP712 {
    bytes32 private constant ATTESTATION_TYPEHASH =
        keccak256("Attestation(address wallet,bytes32 nullifierHash,uint40 expiresAt)");

    address public immutable attester;

    mapping(address wallet => uint40 expiresAt) public humanUntil;

    /// @notice One World ID nullifier can only ever back one wallet, so a
    /// single person cannot mint themselves an unlimited supply of free
    /// senders.
    mapping(bytes32 nullifierHash => address wallet) public nullifierOwner;

    event HumanAttested(address indexed wallet, bytes32 indexed nullifierHash, uint40 expiresAt);

    error InvalidSignature();
    error AttestationExpired();
    error NullifierAlreadyBound(address boundTo);
    error NotAnExtension(uint40 current);

    constructor(address attester_) EIP712("Postage", "1") {
        if (attester_ == address(0)) revert InvalidSignature();
        attester = attester_;
    }

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

    function isHuman(address wallet) external view returns (bool) {
        return humanUntil[wallet] > block.timestamp;
    }

    function domainSeparator() external view returns (bytes32) {
        return _domainSeparatorV4();
    }
}
