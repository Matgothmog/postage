// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/// @notice Holds the protocol's share of postage that recipients claimed from
/// spam, and spends it making verification free.
///
/// Verified senders pay no postage, but posting their attestation still costs
/// gas, so "free" was not quite true. This vault closes that gap: the share
/// taken from spam refills the relayer that submits attestations on people's
/// behalf. Spam pays for everyone else's onboarding.
///
/// Only claimed stamps ever reach here. Refunds to good-faith senders are
/// untouched, because a bond you do not get back in full is just a fee.
contract PostageVault {
    /// @notice Share of incoming funds kept for the protocol. The rest pays gas.
    uint16 public constant TREASURY_BPS = 3_000;
    uint16 private constant ONE = 10_000;

    address public immutable treasury;
    /// @notice The account that submits attestations for users. It holds no
    /// authority: it can only ever be topped up, never drained to elsewhere.
    address public immutable relayer;

    uint256 public sponsorshipPool;
    uint256 public treasuryBalance;

    event Funded(uint256 amount, uint256 toTreasury, uint256 toSponsorship);
    event RelayerRefilled(uint256 amount);
    event TreasuryWithdrawn(address indexed to, uint256 amount);

    error NotTreasury();
    error InsufficientPool(uint256 available, uint256 requested);
    error TransferFailed();
    error ZeroAddress();

    constructor(address treasury_, address relayer_) {
        if (treasury_ == address(0) || relayer_ == address(0)) revert ZeroAddress();
        treasury = treasury_;
        relayer = relayer_;
    }

    receive() external payable {
        uint256 toTreasury = (msg.value * TREASURY_BPS) / ONE;
        uint256 toSponsorship = msg.value - toTreasury;

        treasuryBalance += toTreasury;
        sponsorshipPool += toSponsorship;

        emit Funded(msg.value, toTreasury, toSponsorship);
    }

    /// @notice Anyone may trigger a refill, because the funds can only go to
    /// the relayer. That keeps the vault usable by a keeper without handing
    /// anybody the ability to move money somewhere else.
    function refillRelayer(uint256 amount) external {
        if (amount > sponsorshipPool) revert InsufficientPool(sponsorshipPool, amount);
        sponsorshipPool -= amount;

        emit RelayerRefilled(amount);
        _pay(relayer, amount);
    }

    function withdrawTreasury(address to, uint256 amount) external {
        if (msg.sender != treasury) revert NotTreasury();
        if (to == address(0)) revert ZeroAddress();
        if (amount > treasuryBalance) revert InsufficientPool(treasuryBalance, amount);
        treasuryBalance -= amount;

        emit TreasuryWithdrawn(to, amount);
        _pay(to, amount);
    }

    function _pay(address to, uint256 amount) private {
        (bool ok,) = to.call{value: amount}("");
        if (!ok) revert TransferFailed();
    }
}
