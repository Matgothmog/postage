// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ECDSA} from "openzeppelin/utils/cryptography/ECDSA.sol";
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

    function _sign(
        uint256 key,
        bytes32 messageId,
        address inbox,
        PostageEscrow.Tier tier,
        uint256 amount,
        uint40 expiresAt
    ) private view returns (bytes memory) {
        return _signFor(escrow, key, messageId, inbox, tier, amount, expiresAt);
    }

    /// A quote is bound to the escrow that will settle it through the EIP-712
    /// domain, so a test that deploys its own escrow must sign against that one.
    function _signFor(
        PostageEscrow target,
        uint256 key,
        bytes32 messageId,
        address inbox,
        PostageEscrow.Tier tier,
        uint256 amount,
        uint40 expiresAt
    ) private view returns (bytes memory) {
        bytes32 digest = target.quoteDigest(messageId, inbox, tier, amount, expiresAt);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(key, digest);
        return abi.encodePacked(r, s, v);
    }

    /// Who a quote recovers to once one of its signed fields has been changed
    /// underneath it. Computed rather than guessed, so a revert test can name
    /// the exact address the contract will report.
    function _recoveredSigner(
        bytes32 messageId,
        address inbox,
        PostageEscrow.Tier tier,
        uint256 amount,
        uint40 expiresAt,
        bytes memory signature
    ) private view returns (address) {
        return ECDSA.recover(escrow.quoteDigest(messageId, inbox, tier, amount, expiresAt), signature);
    }

    function _expiry() private view returns (uint40) {
        return uint40(block.timestamp + 1 hours);
    }

    function test_paymentAccruesToInboxAndVault() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        uint256 toVault = (CENT * escrow.VAULT_BPS()) / 10_000;
        assertEq(escrow.earnings(alice), CENT - toVault);
        assertEq(address(vault).balance, toVault);
    }

    function test_rejectsPriceNotSignedByAnEnclave() public {
        uint40 expiry = _expiry();
        bytes memory forged = _sign(0xBAD, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);
        address forger = vm.addr(0xBAD);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.UnknownEnclave.selector, forger));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, forged);
    }

    function test_rejectsRevokedEnclave() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(owner);
        registry.revoke(enclave);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.UnknownEnclave.selector, enclave));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    function test_quoteCannotBeReplayedOnAnotherMessage() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        vm.prank(sender);
        vm.expectRevert(PostageEscrow.AlreadySettled.selector);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    function test_staleQuoteIsRejected() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.warp(expiry);

        vm.prank(sender);
        vm.expectRevert(PostageEscrow.QuoteExpired.selector);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    /// The enclave may price above an inbox's floor but never below it.
    function test_quoteBelowInboxFloorIsRejected() public {
        uint40 expiry = _expiry();
        uint256 tooLow = CENT - 1;
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, tooLow, expiry);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.BelowFloor.selector, CENT, tooLow));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, tooLow, expiry, quote);
    }

    function test_underpayingTheQuoteIsRejected() public {
        uint40 expiry = _expiry();
        uint256 quoted = CENT * 5;
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Dangerous, quoted, expiry);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.Underpaid.selector, quoted, CENT));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Dangerous, quoted, expiry, quote);
    }

    /// Tampering with any signed field must invalidate the quote, since the
    /// signature is what makes the price trustworthy.
    function test_tamperedAmountInvalidatesTheQuote() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        address recovered =
            _recoveredSigner(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT * 2, expiry, quote);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.UnknownEnclave.selector, recovered));
        escrow.payToSend{value: CENT * 2}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT * 2, expiry, quote);
    }

    /// Tier is signed too, so a sender cannot buy the cheap classification and
    /// then claim the message was something else.
    function test_tamperedTierInvalidatesTheQuote() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        address recovered =
            _recoveredSigner(MESSAGE, alice, PostageEscrow.Tier.Human, CENT, expiry, quote);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.UnknownEnclave.selector, recovered));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Human, CENT, expiry, quote);
    }

    /// The quote signs over the inbox, so a price bought for one recipient is
    /// worthless against another even while it is still fresh and unspent.
    function test_quoteCannotBeReplayedOnAnotherInbox() public {
        address bob = makeAddr("bob");
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        address recovered =
            _recoveredSigner(MESSAGE, bob, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.UnknownEnclave.selector, recovered));
        escrow.payToSend{value: CENT}(MESSAGE, bob, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    function test_inboxClaimsItsEarningsOnce() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

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
        bytes memory quote = _sign(enclaveKey, MESSAGE, bob, PostageEscrow.Tier.Commercial, 1, expiry);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.BelowFloor.selector, escrow.DEFAULT_FLOOR(), 1));
        escrow.payToSend{value: 1}(MESSAGE, bob, PostageEscrow.Tier.Commercial, 1, expiry, quote);
    }

    /// The subgraph reconstructs what a sender actually paid as `amount +
    /// toVault`, so `amount` has to be the inbox's net share rather than the
    /// quoted price. Overpaying keeps quoted, net and vault share all distinct,
    /// so no reordering or substitution of the three can slip through.
    function test_paidEventReportsTheInboxNetShareRatherThanTheQuotedPrice() public {
        uint40 expiry = _expiry();
        uint256 paid = CENT * 3;
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        uint256 toVault = (paid * escrow.VAULT_BPS()) / 10_000;

        vm.expectEmit(true, true, true, true, address(escrow));
        emit PostageEscrow.Paid(
            MESSAGE, sender, alice, PostageEscrow.Tier.Commercial, paid - toVault, toVault
        );

        vm.prank(sender);
        escrow.payToSend{value: paid}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    /// Reputation is priced off the tier the enclave chose, so the tier that was
    /// signed has to be the tier that reaches the subgraph.
    function test_everyTierRoundTripsIntoThePaidEvent() public {
        uint256 toVault = (CENT * escrow.VAULT_BPS()) / 10_000;

        for (uint256 i = 0; i <= uint256(type(PostageEscrow.Tier).max); i++) {
            PostageEscrow.Tier tier = PostageEscrow.Tier(i);
            bytes32 messageId = keccak256(abi.encode(MESSAGE, i));
            uint40 expiry = _expiry();
            bytes memory quote = _sign(enclaveKey, messageId, alice, tier, CENT, expiry);

            vm.expectEmit(true, true, true, true, address(escrow));
            emit PostageEscrow.Paid(messageId, sender, alice, tier, CENT - toVault, toVault);

            vm.prank(sender);
            escrow.payToSend{value: CENT}(messageId, alice, tier, CENT, expiry, quote);
        }
    }

    /// Earnings credited to the zero address would be unclaimable, so a quote
    /// naming it is refused before any money moves.
    function test_payingAZeroInboxIsRejected() public {
        uint40 expiry = _expiry();
        bytes memory quote =
            _sign(enclaveKey, MESSAGE, address(0), PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        vm.expectRevert(PostageEscrow.ZeroAddress.selector);
        escrow.payToSend{value: CENT}(MESSAGE, address(0), PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    /// Expiry is checked with `>=`, so the second before it is still good and a
    /// quote is not lost to an off-by-one.
    function test_quoteIsStillGoodOneSecondBeforeItExpires() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.warp(expiry - 1);

        vm.prank(sender);
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        assertTrue(escrow.settled(MESSAGE));
    }

    /// The vault is paid inline, so a vault that cannot take its share must undo
    /// the whole payment rather than leave the split half-applied.
    function test_paymentRevertsWhenTheVaultRejectsItsShare() public {
        PostageEscrow brokenEscrow = new PostageEscrow(address(registry), address(new RejectsEther()));

        uint40 expiry = _expiry();
        bytes memory quote =
            _signFor(brokenEscrow, enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        vm.expectRevert(PostageEscrow.TransferFailed.selector);
        brokenEscrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    function test_claimRevertsWhenTheRecipientRejectsEther() public {
        address recipient = address(new RejectsEther());
        _settle(MESSAGE);

        vm.prank(alice);
        vm.expectRevert(PostageEscrow.TransferFailed.selector);
        escrow.claimEarnings(recipient);
    }

    function test_claimingToTheZeroAddressIsRejected() public {
        _settle(MESSAGE);
        uint256 accrued = escrow.earnings(alice);

        vm.prank(alice);
        vm.expectRevert(PostageEscrow.ZeroAddress.selector);
        escrow.claimEarnings(address(0));

        assertEq(escrow.earnings(alice), accrued, "a rejected claim leaves the balance alone");
    }

    /// The caller spends its own ledger but names where the money lands, so the
    /// payer and the payee are separate roles.
    function test_earningsGoToTheNamedRecipientRatherThanTheCaller() public {
        address bob = makeAddr("bob");
        _settle(MESSAGE);
        uint256 accrued = escrow.earnings(alice);

        vm.prank(alice);
        escrow.claimEarnings(bob);

        assertEq(bob.balance, accrued);
        assertEq(alice.balance, 0, "the caller is not paid for naming someone else");
        assertEq(escrow.earnings(alice), 0);
        assertEq(escrow.earnings(bob), 0, "the recipient's own ledger is untouched");
    }

    function test_claimEmitsTheInboxDestinationAndAmount() public {
        address bob = makeAddr("bob");
        _settle(MESSAGE);
        uint256 accrued = escrow.earnings(alice);

        vm.expectEmit(true, true, true, true, address(escrow));
        emit PostageEscrow.EarningsClaimed(alice, bob, accrued);

        vm.prank(alice);
        escrow.claimEarnings(bob);
    }

    function test_settingAFloorEmitsTheInboxAndAmount() public {
        uint256 chosen = CENT * 7;

        vm.expectEmit(true, true, true, true, address(escrow));
        emit PostageEscrow.FloorPriceSet(alice, chosen);

        vm.prank(alice);
        escrow.setFloorPrice(chosen);
    }

    /// A raised floor applies to quotes that are still in flight, not only to
    /// ones issued afterwards.
    function test_raisingTheFloorRejectsAQuoteTheOldFloorAllowed() public {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(alice);
        escrow.setFloorPrice(CENT * 2);

        vm.prank(sender);
        vm.expectRevert(abi.encodeWithSelector(PostageEscrow.BelowFloor.selector, CENT * 2, CENT));
        escrow.payToSend{value: CENT}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    /// A chosen floor is taken literally however low it is. That is what makes
    /// zero a cliff rather than the bottom of a range.
    function test_aFloorBelowTheDefaultIsHonouredAsChosen() public {
        uint256 cheap = 1 wei;

        vm.prank(alice);
        escrow.setFloorPrice(cheap);
        assertEq(escrow.effectiveFloor(alice), cheap);

        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, cheap, expiry);

        vm.prank(sender);
        escrow.payToSend{value: cheap}(MESSAGE, alice, PostageEscrow.Tier.Commercial, cheap, expiry, quote);

        assertTrue(escrow.settled(MESSAGE));
    }

    /// Zero is the "never chose one" sentinel rather than a price, so clearing a
    /// floor quietly restores the default instead of making mail free.
    function test_clearingTheFloorRestoresTheDefaultRatherThanMakingMailFree() public {
        vm.prank(alice);
        escrow.setFloorPrice(0);

        assertEq(escrow.effectiveFloor(alice), escrow.DEFAULT_FLOOR());

        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, 1, expiry);

        vm.prank(sender);
        vm.expectRevert(
            abi.encodeWithSelector(PostageEscrow.BelowFloor.selector, escrow.DEFAULT_FLOOR(), 1)
        );
        escrow.payToSend{value: 1}(MESSAGE, alice, PostageEscrow.Tier.Commercial, 1, expiry, quote);
    }

    function _settle(bytes32 messageId) private {
        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, messageId, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: CENT}(messageId, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);
    }

    function testFuzz_vaultAndInboxAlwaysSplitTheWholePayment(uint96 paid) public {
        vm.assume(paid >= CENT);
        vm.deal(sender, paid);

        uint40 expiry = _expiry();
        bytes memory quote = _sign(enclaveKey, MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry);

        vm.prank(sender);
        escrow.payToSend{value: paid}(MESSAGE, alice, PostageEscrow.Tier.Commercial, CENT, expiry, quote);

        assertEq(escrow.earnings(alice) + address(vault).balance, paid);
    }
}

/// A contract with neither `receive` nor a payable fallback, so every payment to
/// it fails and the caller's TransferFailed branch is reached.
contract RejectsEther {}
