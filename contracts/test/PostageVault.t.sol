// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {PostageVault} from "../src/PostageVault.sol";

contract PostageVaultTest is Test {
    PostageVault internal vault;

    address internal treasury = makeAddr("treasury");
    address internal relayer = makeAddr("relayer");
    address internal anyone = makeAddr("anyone");

    uint256 internal constant FUNDING = 1 ether;

    function setUp() public {
        vault = new PostageVault(treasury, relayer);
        vm.deal(address(this), 10 ether);
    }

    function _fund(uint256 amount) private {
        (bool ok,) = address(vault).call{value: amount}("");
        assertTrue(ok);
    }

    function test_incomingFundsSplitByTreasuryShare() public {
        _fund(FUNDING);

        uint256 expectedTreasury = (FUNDING * vault.TREASURY_BPS()) / 10_000;
        assertEq(vault.treasuryBalance(), expectedTreasury);
        assertEq(vault.sponsorshipPool(), FUNDING - expectedTreasury);
        assertEq(address(vault).balance, FUNDING);
    }

    function test_anyoneCanRefillTheRelayer() public {
        _fund(FUNDING);
        uint256 pool = vault.sponsorshipPool();

        vm.prank(anyone);
        vault.refillRelayer(pool);

        assertEq(relayer.balance, pool, "funds go to the relayer, not the caller");
        assertEq(anyone.balance, 0);
        assertEq(vault.sponsorshipPool(), 0);
    }

    function test_refillCannotExceedSponsorshipPool() public {
        _fund(FUNDING);
        uint256 pool = vault.sponsorshipPool();

        vm.expectRevert(
            abi.encodeWithSelector(PostageVault.InsufficientPool.selector, pool, pool + 1)
        );
        vault.refillRelayer(pool + 1);
    }

    function test_refillCannotReachTreasuryFunds() public {
        _fund(FUNDING);

        vault.refillRelayer(vault.sponsorshipPool());

        assertEq(address(vault).balance, vault.treasuryBalance(), "treasury share is untouched");
    }

    function test_onlyTreasuryWithdraws() public {
        _fund(FUNDING);

        vm.prank(anyone);
        vm.expectRevert(PostageVault.NotTreasury.selector);
        vault.withdrawTreasury(anyone, 1);
    }

    function test_treasuryWithdrawsItsShare() public {
        _fund(FUNDING);
        uint256 balance = vault.treasuryBalance();

        vm.prank(treasury);
        vault.withdrawTreasury(treasury, balance);

        assertEq(treasury.balance, balance);
        assertEq(vault.treasuryBalance(), 0);
    }

    function test_treasuryCannotDipIntoSponsorship() public {
        _fund(FUNDING);
        uint256 balance = vault.treasuryBalance();

        vm.prank(treasury);
        vm.expectRevert(
            abi.encodeWithSelector(PostageVault.InsufficientPool.selector, balance, balance + 1)
        );
        vault.withdrawTreasury(treasury, balance + 1);
    }

    function test_rejectsZeroAddresses() public {
        vm.expectRevert(PostageVault.ZeroAddress.selector);
        new PostageVault(address(0), relayer);

        vm.expectRevert(PostageVault.ZeroAddress.selector);
        new PostageVault(treasury, address(0));
    }

    function test_withdrawingToTheZeroAddressIsRejected() public {
        _fund(FUNDING);
        uint256 balance = vault.treasuryBalance();

        vm.prank(treasury);
        vm.expectRevert(PostageVault.ZeroAddress.selector);
        vault.withdrawTreasury(address(0), balance);

        assertEq(vault.treasuryBalance(), balance, "a rejected withdrawal leaves the ledger alone");
    }

    function test_withdrawRevertsWhenTheRecipientRejectsEther() public {
        address recipient = address(new RejectsEther());
        _fund(FUNDING);
        uint256 balance = vault.treasuryBalance();

        vm.prank(treasury);
        vm.expectRevert(PostageVault.TransferFailed.selector);
        vault.withdrawTreasury(recipient, balance);
    }

    /// The relayer is fixed at construction, so a relayer that cannot take the
    /// funds strands the pool rather than losing it.
    function test_refillRevertsWhenTheRelayerRejectsEther() public {
        PostageVault strandedVault = new PostageVault(treasury, address(new RejectsEther()));
        (bool ok,) = address(strandedVault).call{value: FUNDING}("");
        assertTrue(ok);

        uint256 pool = strandedVault.sponsorshipPool();

        vm.expectRevert(PostageVault.TransferFailed.selector);
        strandedVault.refillRelayer(pool);
    }

    /// A keeper refilling on a timer will hit an empty pool, which is a no-op
    /// rather than an error.
    function test_refillingNothingMovesNothingAndStillEmits() public {
        _fund(FUNDING);
        uint256 pool = vault.sponsorshipPool();

        vm.expectEmit(true, true, true, true, address(vault));
        emit PostageVault.RelayerRefilled(0);

        vault.refillRelayer(0);

        assertEq(vault.sponsorshipPool(), pool);
        assertEq(relayer.balance, 0);
    }

    function test_incomingFundsEmitTheSplitTheyWereRecordedUnder() public {
        uint256 toTreasury = (FUNDING * vault.TREASURY_BPS()) / 10_000;

        vm.expectEmit(true, true, true, true, address(vault));
        emit PostageVault.Funded(FUNDING, toTreasury, FUNDING - toTreasury);

        _fund(FUNDING);
    }

    function test_refillEmitsTheAmountSentToTheRelayer() public {
        _fund(FUNDING);
        uint256 pool = vault.sponsorshipPool();

        vm.expectEmit(true, true, true, true, address(vault));
        emit PostageVault.RelayerRefilled(pool);

        vault.refillRelayer(pool);
    }

    function test_treasuryWithdrawalEmitsTheDestinationAndAmount() public {
        _fund(FUNDING);
        uint256 balance = vault.treasuryBalance();

        vm.expectEmit(true, true, true, true, address(vault));
        emit PostageVault.TreasuryWithdrawn(anyone, balance);

        vm.prank(treasury);
        vault.withdrawTreasury(anyone, balance);
    }

    /// A single deposit cannot catch an accumulation bug, which is the way this
    /// would actually break, so the invariant is checked after each of several
    /// deposits with both ledgers partly drained in between.
    function testFuzz_accountingAlwaysMatchesBalance(
        uint96[4] memory deposits,
        uint96[4] memory drains
    ) public {
        for (uint256 i = 0; i < deposits.length; i++) {
            vm.deal(address(this), deposits[i]);
            _fund(deposits[i]);

            vault.refillRelayer(drains[i] % (vault.sponsorshipPool() + 1));

            uint256 fromTreasury = drains[i] % (vault.treasuryBalance() + 1);
            vm.prank(treasury);
            vault.withdrawTreasury(treasury, fromTreasury);

            assertEq(vault.treasuryBalance() + vault.sponsorshipPool(), address(vault).balance);
        }
    }
}

/// A contract with neither `receive` nor a payable fallback, so every payment to
/// it fails and the caller's TransferFailed branch is reached.
contract RejectsEther {}
