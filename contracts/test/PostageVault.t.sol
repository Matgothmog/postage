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

    function testFuzz_accountingAlwaysMatchesBalance(uint96 amount) public {
        vm.assume(amount > 0);
        vm.deal(address(this), amount);
        _fund(amount);

        assertEq(vault.treasuryBalance() + vault.sponsorshipPool(), address(vault).balance);
    }
}
