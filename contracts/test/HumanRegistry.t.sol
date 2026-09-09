// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {HumanRegistry} from "../src/HumanRegistry.sol";

contract HumanRegistryTest is Test {
    HumanRegistry internal registry;

    uint256 internal attesterKey = 0xA11CE;
    address internal attester;

    address internal wallet = makeAddr("wallet");
    address internal other = makeAddr("other");

    bytes32 internal constant NULLIFIER = keccak256("nullifier");
    uint40 internal constant NINETY_DAYS = 90 days;

    function setUp() public {
        attester = vm.addr(attesterKey);
        registry = new HumanRegistry(attester);
    }

    function _sign(uint256 key, address wallet_, bytes32 nullifier, uint40 expiresAt)
        private
        view
        returns (bytes memory)
    {
        bytes32 structHash = keccak256(
            abi.encode(
                keccak256("Attestation(address wallet,bytes32 nullifierHash,uint40 expiresAt)"),
                wallet_,
                nullifier,
                expiresAt
            )
        );
        bytes32 digest =
            keccak256(abi.encodePacked("\x19\x01", registry.domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(key, digest);
        return abi.encodePacked(r, s, v);
    }

    function _expiry() private view returns (uint40) {
        return uint40(block.timestamp) + NINETY_DAYS;
    }

    function test_attestMarksWalletHuman() public {
        uint40 expiresAt = _expiry();
        registry.attest(wallet, NULLIFIER, expiresAt, _sign(attesterKey, wallet, NULLIFIER, expiresAt));

        assertTrue(registry.isHuman(wallet));
        assertEq(registry.humanUntil(wallet), expiresAt);
        assertEq(registry.nullifierOwner(NULLIFIER), wallet);
    }

    function test_anyoneCanSubmitSomeoneElsesAttestation() public {
        uint40 expiresAt = _expiry();
        bytes memory signature = _sign(attesterKey, wallet, NULLIFIER, expiresAt);

        vm.prank(other);
        registry.attest(wallet, NULLIFIER, expiresAt, signature);

        assertTrue(registry.isHuman(wallet));
    }

    function test_credentialLapsesAfterNinetyDays() public {
        uint40 expiresAt = _expiry();
        registry.attest(wallet, NULLIFIER, expiresAt, _sign(attesterKey, wallet, NULLIFIER, expiresAt));

        vm.warp(expiresAt);
        assertFalse(registry.isHuman(wallet));
    }

    function test_rejectsSignatureFromAnotherKey() public {
        uint40 expiresAt = _expiry();
        bytes memory wrongKey = _sign(0xBAD, wallet, NULLIFIER, expiresAt);

        vm.expectRevert(HumanRegistry.InvalidSignature.selector);
        registry.attest(wallet, NULLIFIER, expiresAt, wrongKey);
    }

    function test_oneNullifierCannotBackTwoWallets() public {
        uint40 expiresAt = _expiry();
        registry.attest(wallet, NULLIFIER, expiresAt, _sign(attesterKey, wallet, NULLIFIER, expiresAt));

        bytes memory reused = _sign(attesterKey, other, NULLIFIER, expiresAt);

        vm.expectRevert(
            abi.encodeWithSelector(HumanRegistry.NullifierAlreadyBound.selector, wallet)
        );
        registry.attest(other, NULLIFIER, expiresAt, reused);
    }

    function test_rejectsAlreadyExpiredAttestation() public {
        vm.warp(1000);
        uint40 stale = uint40(block.timestamp);
        bytes memory signature = _sign(attesterKey, wallet, NULLIFIER, stale);

        vm.expectRevert(HumanRegistry.AttestationExpired.selector);
        registry.attest(wallet, NULLIFIER, stale, signature);
    }

    function test_renewalExtendsCredential() public {
        uint40 first = _expiry();
        registry.attest(wallet, NULLIFIER, first, _sign(attesterKey, wallet, NULLIFIER, first));

        vm.warp(block.timestamp + 60 days);
        uint40 second = _expiry();
        registry.attest(wallet, NULLIFIER, second, _sign(attesterKey, wallet, NULLIFIER, second));

        assertEq(registry.humanUntil(wallet), second);
    }

    function test_oldSignatureCannotShortenRenewedCredential() public {
        uint40 shortExpiry = _expiry();
        bytes memory old = _sign(attesterKey, wallet, NULLIFIER, shortExpiry);

        uint40 longExpiry = shortExpiry + 30 days;
        registry.attest(wallet, NULLIFIER, longExpiry, _sign(attesterKey, wallet, NULLIFIER, longExpiry));

        vm.expectRevert(
            abi.encodeWithSelector(HumanRegistry.NotAnExtension.selector, longExpiry)
        );
        registry.attest(wallet, NULLIFIER, shortExpiry, old);
    }

    function test_unknownWalletIsNotHuman() public view {
        assertFalse(registry.isHuman(other));
    }

    /// The extension check is `<=`, so re-submitting the exact same expiry is
    /// refused rather than being a harmless no-op.
    function test_reAttestingWithTheSameExpiryIsNotAnExtension() public {
        uint40 expiresAt = _expiry();
        bytes memory signature = _sign(attesterKey, wallet, NULLIFIER, expiresAt);
        registry.attest(wallet, NULLIFIER, expiresAt, signature);

        vm.expectRevert(abi.encodeWithSelector(HumanRegistry.NotAnExtension.selector, expiresAt));
        registry.attest(wallet, NULLIFIER, expiresAt, signature);
    }

    function test_attestationEmitsTheWalletNullifierAndExpiry() public {
        uint40 expiresAt = _expiry();

        vm.expectEmit(true, true, true, true, address(registry));
        emit HumanRegistry.HumanAttested(wallet, NULLIFIER, expiresAt);

        registry.attest(wallet, NULLIFIER, expiresAt, _sign(attesterKey, wallet, NULLIFIER, expiresAt));
    }
}
