// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/// @notice Records which enclave signing keys the protocol will accept prices
/// from.
///
/// The point of this contract is that a price is only valid if it came from
/// code whose hash is public. An enclave generates a keypair at boot, the
/// public half appears in its attestation document alongside a measurement of
/// the image that produced it, and that key is registered here. Everything
/// downstream can then treat an enclave signature as proof of provenance.
///
/// Registration is owner-gated for now. The measurement is recorded from the
/// start so that verifying attestations onchain is a change of gatekeeper
/// rather than a change of interface.
contract EnclaveRegistry {
    address public immutable owner;

    /// @notice PCR0 of the image allowed to sign. Zero until an image ships.
    bytes32 public expectedMeasurement;

    mapping(address signer => bool allowed) public isRegistered;
    mapping(address signer => bytes32 measurement) public measurementOf;

    event MeasurementSet(bytes32 measurement);
    event EnclaveRegistered(address indexed signer, bytes32 measurement);
    event EnclaveRevoked(address indexed signer);

    error NotOwner();
    error ZeroAddress();
    error NoMeasurementSet();
    error AlreadyRegistered();

    constructor(address owner_) {
        if (owner_ == address(0)) revert ZeroAddress();
        owner = owner_;
    }

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    /// @notice Publishes the image hash that registered enclaves must match.
    /// Anyone can recompute this from a clean checkout and compare.
    function setMeasurement(bytes32 measurement) external onlyOwner {
        expectedMeasurement = measurement;
        emit MeasurementSet(measurement);
    }

    function register(address signer) external onlyOwner {
        if (signer == address(0)) revert ZeroAddress();
        if (expectedMeasurement == bytes32(0)) revert NoMeasurementSet();
        if (isRegistered[signer]) revert AlreadyRegistered();

        isRegistered[signer] = true;
        measurementOf[signer] = expectedMeasurement;

        emit EnclaveRegistered(signer, expectedMeasurement);
    }

    /// @notice Enclave keys are ephemeral per boot, so revoking a retired one
    /// keeps the accepted set honest.
    function revoke(address signer) external onlyOwner {
        isRegistered[signer] = false;
        emit EnclaveRevoked(signer);
    }
}
