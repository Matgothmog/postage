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
    /// @notice Which measurement each registered signer was recorded against.
    /// Unused by any on-chain caller, web, worker, or subgraph beyond tests as
    /// of this writing — `isRegistered` is the only check anything makes.
    /// Kept rather than removed: this is a mapping slot, and removing it
    /// changes this contract's storage layout, which a deployed contract
    /// cannot survive. It also stays ready for whenever attestations are
    /// verified on-chain, which is the reason it was written in the first
    /// place — see the contract header.
    mapping(address signer => bytes32 measurement) public measurementOf;

    /// @notice The image hash registered enclaves must match was changed.
    event MeasurementSet(bytes32 measurement);
    /// @notice `signer` was added to the accepted set, against the given
    /// measurement.
    event EnclaveRegistered(address indexed signer, bytes32 measurement);
    /// @notice `signer` was removed from the accepted set. Emitted whenever
    /// `revoke` is called, including for a signer that was never registered or
    /// already revoked — see `revoke`.
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

    /// @notice Adds `signer` to the accepted set against the currently
    /// published measurement.
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
    ///
    /// Permissive on purpose in the failure cases, not by oversight: there is
    /// no `isRegistered[signer]` guard, so this succeeds and emits
    /// `EnclaveRevoked` for a signer that was never registered, and again for
    /// one already revoked. There is also no zero-address guard. Because the
    /// subgraph is built purely from events, a stray call here can materialise
    /// a phantom enclave entity or double-count a revocation downstream — the
    /// owner is trusted not to make that call.
    function revoke(address signer) external onlyOwner {
        isRegistered[signer] = false;
        emit EnclaveRevoked(signer);
    }
}
