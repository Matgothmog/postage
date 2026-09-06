// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {EnclaveRegistry} from "../src/EnclaveRegistry.sol";
import {PostageEscrow} from "../src/PostageEscrow.sol";
import {PostageVault} from "../src/PostageVault.sol";

contract PostageEscrowTest is Test {
    EnclaveRegistry internal registry;
    PostageEscrow internal escrow;
    PostageVault internal vault;

    uint256 internal enclaveKey = 0xE1C1A7E;
    address internal enclave;

    address internal owner = makeAddr("owner");
    address internal treasury = makeAddr("treasury");
    address internal relayer = makeAddr("relayer");
    address internal alice = makeAddr("alice");
    address internal sender = makeAddr("sender");

    bytes32 internal constant MESSAGE = keccak256("subject: newsletter");
    bytes32 internal constant MEASUREMENT = keccak256("pcr0");
    uint256 internal constant CENT = 0.01 ether;

    function setUp() public {
        enclave = vm.addr(enclaveKey);

        vault = new PostageVault(treasury, relayer);
        registry = new EnclaveRegistry(owner);
        escrow = new PostageEscrow(address(registry), address(vault));

        vm.startPrank(owner);
        registry.setMeasurement(MEASUREMENT);
        registry.register(enclave);
        vm.stopPrank();

        vm.prank(alice);
        escrow.setFloorPrice(CENT);

        vm.deal(sender, 1 ether);
    }

    function _sign(uint256 key, bytes32 messageId, PostageEscrow.Tier tier, uint256 amount, uint40 expiresAt)
        private
        view
        returns (bytes memory)
    {
        bytes32 digest = escrow.quoteDigest(messageId, alice, tier, amount, expiresAt);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(key, digest);
        return abi.encodePacked(r, s, v);
    }

    function _expiry() private view returns (uint40) {
        return uint40(block.timestamp + 1 hours);
    }

    function test_paymentAccruesToInboxAndVault() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        uint256 toVault = (CENT * escrow.VAULT_BPS()) / 10_000;
        assertEq(escrow.earnings(alice), CENT - toVault);
        assertEq(address(vault).balance, toVault);
    }

    function test_rejectsPriceNotSignedByAnEnclave() public {
        uint40 expiry = _expiry();
        bytes memory forged = _sign(0xBAD, MESSAGE, PostageEscrow.Tier.Commercial, CENT, expiry);
        address forger = vm.addr(0xBAD);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.UnknownEnclave.selector, forger));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, forged);
    }

    function test_rejectsRevokedEnclave() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(owner);
        registry.revoke(enclave);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.UnknownEnclave.selector, enclave));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    function test_quoteCannotBeReplayedOnAnotherMessage() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        vm.prank(sender);
        vm.expectRevert(PostageEscrow.AlreadySettled.selector);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    function test_staleQuoteIsRejected() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.warp(expiry);

        vm.prank(sender);
        vm.expectRevert(PostageEscrow.QuoteExpired.selector);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    /// The enclave may price above an inbox's floor but never below it.
    function test_quoteBelowInboxFloorIsRejected() public {
        uint40 expiry = _expiry();
        uint256 tooLow = CENT - 1;
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Commercial, tooLow, expiry);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.BelowFloor.selector, CENT, tooLow));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, tooLow, expiry, quote);
    }

    function test_underpayingTheQuoteIsRejected() public {
        uint40 expiry = _expiry();
        uint256 quoted = CENT * 5;
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Dangerous, quoted, expiry);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.Underpaid.selector, quoted, CENT));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Dangerous, quoted, expiry, quote);
    }

    /// Tampering with any signed field must invalidate the quote, since the
    /// signature is what makes the price trustworthy.
    function test_tamperedAmountInvalidatesTheQuote() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        vm.expectRevert();
        escrow.payToSend{value: CENT * 2}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT * 2, expiry, quote);
    }

    function test_inboxClaimsItsEarningsOnce() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        uint256 accrued = escrow.earnings(alice);
        vm.prank(alice);
        escrow.claimEarnings(alice);

        assertEq(alice.balance, accrued);
        assertEq(escrow.earnings(alice), 0);

        vm.prank(alice);
        vm.expectRevert(PostageEscrow.NothingToClaim.selector);
        escrow.claimEarnings(alice);
    }

    function test_spamReportNeedsASettledMessage() public {
        vm.prank(alice);
        vm.expectRevert(PostageEscrow.NotSettled.selector);
        escrow.reportSpam(MESSAGE);
    }

    function test_onlyTheRecipientCanReportSpam() public {
        _settle(MESSAGE);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.NotTheRecipient.selector, alice));
        escrow.reportSpam(MESSAGE);
    }

    function test_spamIsReportedAgainstThePayerAndOnlyOnce() public {
        _settle(MESSAGE);

        vm.expectEmit(true, true, true, false);
        emit PostageEscrow.SpamReported(MESSAGE, alice, sender);
        vm.prank(alice);
        escrow.reportSpam(MESSAGE);

        vm.prank(alice);
        vm.expectRevert(PostageEscrow.AlreadyReported.selector);
        escrow.reportSpam(MESSAGE);
    }

    function test_anInboxWithNoChosenPriceStillChargesTheDefault() public {
        address bob = makeAddr("bob");
        assertEq(escrow.floorPrice(bob), 0);
        assertEq(escrow.effectiveFloor(bob), escrow.DEFAULT_FLOOR());

        uint40 expiry = _expiry();
        bytes32 digest = escrow.quoteDigest(MESSAGE, bob, PostageEscrow.Tier.Commercial, 1, expiry);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclaveKey, digest);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.BelowFloor.selector, escrow.DEFAULT_FLOOR(), 1));
        escrow.payToSend{value: 1}(
            MESSAGE, bob, PostageEscrow.Tier.Commercial, 1, expiry, abi.encodePacked(r, s, v)
        );
    }

    function _settle(bytes32 messageId) private {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, messageId, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: CENT}(messageId, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    function testFuzz_vaultAndInboxAlwaysSplitTheWholePayment(uint96 paid) public {
        vm.assume(paid >= CENT);
        vm.deal(sender, paid);

        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: paid}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        assertEq(escrow.earnings(alice) + address(vault).balance, paid);
    }
}
