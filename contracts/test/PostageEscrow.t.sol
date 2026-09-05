// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {PostageEscrow} from "../src/PostageEscrow.sol";

contract PostageEscrowTest is Test {
    PostageEscrow internal escrow;

    address internal alice = makeAddr("alice");
    address internal bob = makeAddr("bob");
    address internal mallory = makeAddr("mallory");

    bytes32 internal constant MESSAGE = keccak256("subject: hello");
    uint256 internal constant CENT = 0.01 ether;

    function setUp() public {
        escrow = new PostageEscrow();
        vm.deal(bob, 1 ether);
        vm.deal(mallory, 1 ether);

        vm.prank(alice);
        escrow.setPrice(CENT);
    }

    function _post(address sender, bytes32 messageId) private {
        vm.prank(sender);
        escrow.postStamp{value: CENT}(messageId, alice);
    }

    function test_releaseRefundsSender() public {
        _post(bob, MESSAGE);
        assertEq(bob.balance, 1 ether - CENT);

        vm.prank(alice);
        escrow.release(MESSAGE);

        assertEq(bob.balance, 1 ether);
        assertEq(alice.balance, 0);
    }

    function test_claimPaysRecipient() public {
        _post(bob, MESSAGE);

        vm.prank(alice);
        escrow.claim(MESSAGE);

        assertEq(alice.balance, CENT);
        assertEq(bob.balance, 1 ether - CENT);
    }

    function test_expireRefundsSenderAfterFourteenDays() public {
        _post(bob, MESSAGE);

        vm.warp(block.timestamp + 14 days);
        escrow.expire(MESSAGE);

        assertEq(bob.balance, 1 ether);
    }

    function test_expireRevertsBeforeDeadline() public {
        _post(bob, MESSAGE);

        vm.warp(block.timestamp + 14 days - 1);
        vm.expectRevert(PostageEscrow.NotYetExpired.selector);
        escrow.expire(MESSAGE);
    }

    function test_onlyRecipientCanRelease() public {
        _post(bob, MESSAGE);

        vm.prank(mallory);
        vm.expectRevert(PostageEscrow.NotRecipient.selector);
        escrow.release(MESSAGE);
    }

    function test_onlyRecipientCanClaim() public {
        _post(bob, MESSAGE);

        vm.prank(mallory);
        vm.expectRevert(PostageEscrow.NotRecipient.selector);
        escrow.claim(MESSAGE);
    }

    function test_underpaidStampReverts() public {
        vm.prank(bob);
        vm.expectRevert(
            abi.encodeWithSelector(PostageEscrow.PostageTooLow.selector, CENT, CENT - 1)
        );
        escrow.postStamp{value: CENT - 1}(MESSAGE, alice);
    }

    function test_cannotReuseMessageId() public {
        _post(bob, MESSAGE);

        vm.prank(mallory);
        vm.expectRevert(PostageEscrow.StampAlreadyExists.selector);
        escrow.postStamp{value: CENT}(MESSAGE, alice);
    }

    function test_cannotSettleTwice() public {
        _post(bob, MESSAGE);

        vm.startPrank(alice);
        escrow.claim(MESSAGE);
        vm.expectRevert(PostageEscrow.StampNotHeld.selector);
        escrow.release(MESSAGE);
        vm.stopPrank();
    }

    function test_inboxWithoutPriceAcceptsAnything() public {
        vm.prank(bob);
        escrow.postStamp{value: 1 wei}(MESSAGE, mallory);

        (,, uint256 amount,,) = escrow.stamps(MESSAGE);
        assertEq(amount, 1 wei);
    }

    function testFuzz_releaseAlwaysReturnsFullAmount(uint96 amount) public {
        vm.assume(amount >= CENT);
        vm.deal(bob, amount);

        vm.prank(bob);
        escrow.postStamp{value: amount}(MESSAGE, alice);
        assertEq(bob.balance, 0);

        vm.prank(alice);
        escrow.release(MESSAGE);
        assertEq(bob.balance, amount);
    }
}
