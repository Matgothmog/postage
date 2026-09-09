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
///
/// Amounts here are native USDC too, at its 18-decimal representation — the
/// same unit PostageEscrow forwards through `_pay`. `sponsorshipPool` and
/// `treasuryBalance` are counted in that unit, not the 6-decimal ERC-20 view.
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

    /// @notice ETH arrived via `receive()` and was split between the treasury
    /// and the sponsorship pool.
    event Funded(uint256 amount, uint256 toTreasury, uint256 toSponsorship);
    /// @notice The relayer was topped up by `amount` from the sponsorship pool.
    event RelayerRefilled(uint256 amount);
    /// @notice The treasury withdrew `amount` of its own accrued share to `to`.
    event TreasuryWithdrawn(address indexed to, uint256 amount);

    error NotTreasury();
    /// @notice Thrown by both `refillRelayer` (when `sponsorshipPool` is
    /// short) and `withdrawTreasury` (when `treasuryBalance` is short) — the
    /// name reflects only the first use. See the throw site in
    /// `withdrawTreasury` for the second; it cannot be renamed without
    /// changing this contract's deployed interface.
    error InsufficientPool(uint256 available, uint256 requested);
    error TransferFailed();
    error ZeroAddress();

    /// @notice `relayer_` is immutable and unrecoverable if wrong: if it
    /// cannot accept a plain ETH transfer, `refillRelayer` reverts forever,
    /// and since `withdrawTreasury` can only ever reach `treasuryBalance`,
    /// never `sponsorshipPool`, the entire sponsorship pool would be
    /// permanently stranded in this contract.
    constructor(address treasury_, address relayer_) {
        if (treasury_ == address(0) || relayer_ == address(0)) revert ZeroAddress();
        treasury = treasury_;
        relayer = relayer_;
    }

    /// @notice Splits incoming ETH between the treasury and the sponsorship
    /// pool. The intended way funds arrive: PostageEscrow's `_pay` sends the
    /// vault's cut here as a plain transfer.
    ///
    /// This function is the entire accounting story behind the invariant the
    /// fuzz tests check, `treasuryBalance + sponsorshipPool ==
    /// address(this).balance` — and it only holds for ETH that passes through
    /// here. ETH forced in via `selfdestruct` or received as a coinbase
    /// payment bypasses `receive()` entirely (there is no `fallback` either),
    /// so it lands in this contract's balance permanently unaccounted and
    /// stuck; there is no sweep. Left unfixed deliberately — a fix would mean
    /// redeploying a vault that already holds funds.
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

        // State updated and event emitted before the external call below —
        // there is no ReentrancyGuard; this ordering is what stops a
        // reentrant call from draining the pool twice against one balance
        // check. Do not reorder relative to `_pay`.
        emit RelayerRefilled(amount);
        _pay(relayer, amount);
    }

    /// @notice Sends `amount` of the treasury's own accrued share to `to`.
    /// Callable only by the treasury address itself.
    function withdrawTreasury(address to, uint256 amount) external {
        if (msg.sender != treasury) revert NotTreasury();
        if (to == address(0)) revert ZeroAddress();
        // Despite the error's name, this checks treasuryBalance, not the
        // sponsorship pool — see the note on InsufficientPool above.
        if (amount > treasuryBalance) revert InsufficientPool(treasuryBalance, amount);
        treasuryBalance -= amount;

        // Same ordering-as-reentrancy-guard reasoning as refillRelayer: state
        // updated and the event emitted before the external call.
        emit TreasuryWithdrawn(to, amount);
        _pay(to, amount);
    }

    function _pay(address to, uint256 amount) private {
        (bool ok,) = to.call{value: amount}("");
        if (!ok) revert TransferFailed();
    }
}
