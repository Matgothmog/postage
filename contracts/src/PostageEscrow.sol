// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {EIP712} from "openzeppelin/utils/cryptography/EIP712.sol";
import {ECDSA} from "openzeppelin/utils/cryptography/ECDSA.sol";
import {EnclaveRegistry} from "./EnclaveRegistry.sol";

/// @notice Collects what a sender pays to reach an inbox, and accrues it to the
/// person who owns that inbox.
///
/// The price is not ours to choose at call time. It is quoted by an enclave
/// that read the message and decided what it was, and this contract will not
/// accept a payment unless that quote carries a signature from a key registered
/// in EnclaveRegistry. A price therefore cannot exist without code whose hash
/// is public having produced it.
///
/// Amounts are native USDC, which on Arc is the gas token, at its 18-decimal
/// representation. The 6-decimal ERC-20 view of the same balance is never used.
contract PostageEscrow is EIP712 {
    /// @notice What the enclave decided a message was. Mirrors the classifier.
    enum Tier {
        Human,
        Important,
        Commercial,
        Dangerous
    }

    bytes32 private constant QUOTE_TYPEHASH = keccak256(
        "Quote(bytes32 messageId,address inbox,uint8 tier,uint256 amount,uint40 expiresAt)"
    );

    /// @notice Share of each payment funding the vault that pays gas for people
    /// who verify. The rest accrues to the inbox.
    uint16 public constant VAULT_BPS = 2_000;
    uint16 private constant ONE = 10_000;

    /// @notice What an inbox charges before its owner has said otherwise. A
    /// wallet that has only ever claimed a handle is protected from the first
    /// message onward, so setting a price is a preference rather than a step
    /// in signing up. Native USDC is 18 decimals here, so this is one cent.
    uint256 public constant DEFAULT_FLOOR = 0.01 ether;

    EnclaveRegistry public immutable registry;
    address public immutable vault;

    /// @notice Floor an inbox owner will accept, or zero while they have not
    /// chosen one. Read `effectiveFloor` rather than this. The enclave may
    /// quote above the floor for a risky sender, never below.
    mapping(address inbox => uint256 amount) public floorPrice;

    mapping(address inbox => uint256 amount) public earnings;

    /// @notice What a settled message was: who received it and who paid.
    /// Recorded so a spam report can be checked against the payment rather
    /// than taken on the caller's word.
    struct Settlement {
        address inbox;
        bool reported;
        address payer;
    }

    /// @notice One payment per message, so a quote cannot be replayed.
    mapping(bytes32 messageId => Settlement) public settlementOf;

    /// @notice An inbox set (or reset) the price it charges. Zero does not
    /// mean it now accepts free mail; it means the override was cleared and
    /// `effectiveFloor` falls back to `DEFAULT_FLOOR` — see `setFloorPrice`.
    event FloorPriceSet(address indexed inbox, uint256 amount);
    /// @notice A quote was settled. `amount` is the inbox's net share of what
    /// was actually sent (`msg.value - toVault`), not the quoted price that
    /// authorized the payment — the two differ whenever a sender overpays,
    /// because the excess is kept and split rather than refunded. See the
    /// overpayment note on `payToSend`.
    event Paid(
        bytes32 indexed messageId,
        address indexed sender,
        address indexed inbox,
        Tier tier,
        uint256 amount,
        uint256 toVault
    );
    /// @notice Recipient disagreeing with the classifier after the fact. Moves
    /// no money; it is reputation the subgraph picks up.
    event SpamReported(bytes32 indexed messageId, address indexed inbox, address indexed sender);
    /// @notice An inbox owner withdrew their accrued earnings to `to`.
    event EarningsClaimed(address indexed inbox, address indexed to, uint256 amount);

    error AlreadySettled();
    error NotSettled();
    error AlreadyReported();
    error NotTheRecipient(address inbox);
    error QuoteExpired();
    error UnknownEnclave(address signer);
    error BelowFloor(uint256 required, uint256 quoted);
    error Underpaid(uint256 quoted, uint256 provided);
    error NothingToClaim();
    error ZeroAddress();
    error TransferFailed();

    /// @notice `vault_` is immutable and unrecoverable if wrong: if it cannot
    /// accept a plain ETH transfer, every `payToSend` call reverts forever at
    /// `_pay` below, because the vault's cut has nowhere else to go.
    ///
    /// EIP-712 version is "2" here against HumanRegistry's "1", not by
    /// accident: it was bumped when this escrow was rewritten from a
    /// hold/release model to the current quote model, so a signature over the
    /// old domain cannot be replayed against this one. Off-chain signers
    /// hardcode this string (web/src/lib/quote.ts) — changing it here without
    /// changing them breaks every quote signature silently.
    constructor(address registry_, address vault_) EIP712("Postage", "2") {
        if (registry_ == address(0) || vault_ == address(0)) revert ZeroAddress();
        registry = EnclaveRegistry(registry_);
        vault = vault_;
    }

    /// @notice Sets the caller's floor price. Passing zero does not make mail
    /// free: it clears the override, and `effectiveFloor` falls back to
    /// `DEFAULT_FLOOR` per the `floorPrice` mapping above. There is no way to
    /// charge literally nothing through this function.
    function setFloorPrice(uint256 amount) external {
        floorPrice[msg.sender] = amount;
        emit FloorPriceSet(msg.sender, amount);
    }

    /// @notice What this inbox actually charges. Zero means the owner never
    /// picked a price, not that mail to them is free.
    function effectiveFloor(address inbox) public view returns (uint256) {
        uint256 chosen = floorPrice[inbox];
        return chosen == 0 ? DEFAULT_FLOOR : chosen;
    }

    /// @notice Whether this message has already been paid for.
    /// @dev Kept as its own function rather than inlined at call sites: the
    /// escrow this replaced exposed `settled(bytes32)` over its own storage,
    /// and this preserves that exact signature over `settlementOf` after the
    /// rewrite to the quote model, so nothing calling it needed to change.
    function settled(bytes32 messageId) public view returns (bool) {
        return settlementOf[messageId].inbox != address(0);
    }

    /// @notice Settles a quoted message: verifies the enclave's signature over
    /// the quote, requires at least the quoted amount, and splits what was
    /// actually sent between the inbox and the vault.
    ///
    /// Overpaying is accepted and never refunded — only `msg.value < amount`
    /// is rejected below, nothing caps the upside — because a refund would
    /// need a second external call this function has no other reason to make.
    /// The split is computed from `msg.value`, not from `amount`, so any
    /// excess over the quote still flows to the inbox and the vault in the
    /// usual proportion; it just means `Paid.amount` and `Paid.toVault` can
    /// exceed what the quote named.
    /// @param messageId Identifies the message this quote was issued for, and
    /// doubles as the replay key checked via `settled`.
    /// @param inbox The recipient the enclave quoted this message for.
    /// @param tier What the enclave classified this message as.
    /// @param amount The quoted price — the minimum that must be sent, not
    /// necessarily what ends up collected; see the overpayment note above.
    /// @param expiresAt Quote deadline; `block.timestamp >= expiresAt` is
    /// rejected, so the boundary instant itself is already expired.
    /// @param enclaveSignature EIP-712 signature over the quote, from a key
    /// registered in EnclaveRegistry.
    function payToSend(
        bytes32 messageId,
        address inbox,
        Tier tier,
        uint256 amount,
        uint40 expiresAt,
        bytes calldata enclaveSignature
    ) external payable {
        if (inbox == address(0)) revert ZeroAddress();
        if (settled(messageId)) revert AlreadySettled();
        if (block.timestamp >= expiresAt) revert QuoteExpired();

        uint256 floor = effectiveFloor(inbox);
        if (amount < floor) revert BelowFloor(floor, amount);
        if (msg.value < amount) revert Underpaid(amount, msg.value);

        address signer = _recoverQuoteSigner(messageId, inbox, tier, amount, expiresAt, enclaveSignature);
        if (!registry.isRegistered(signer)) revert UnknownEnclave(signer);

        // Settlement recorded, earnings credited, and the event emitted
        // before the external call below. There is no ReentrancyGuard here;
        // this ordering is the only thing stopping a reentrant call from
        // re-settling this message or double-crediting the inbox. Do not
        // reorder these lines relative to `_pay`.
        settlementOf[messageId] = Settlement({inbox: inbox, reported: false, payer: msg.sender});

        uint256 toVault = (msg.value * VAULT_BPS) / ONE;
        earnings[inbox] += msg.value - toVault;

        emit Paid(messageId, msg.sender, inbox, tier, msg.value - toVault, toVault);
        _pay(vault, toVault);
    }

    /// @notice The classifier let something through that should not have been.
    /// Recorded rather than refunded: the money has already accrued, and what
    /// matters is that this sender is priced worse next time.
    ///
    /// Only the inbox that received the message may report it, and only once.
    /// Reputation that anyone could write to would price nobody correctly.
    function reportSpam(bytes32 messageId) external {
        Settlement storage settlement = settlementOf[messageId];
        if (settlement.inbox == address(0)) revert NotSettled();
        if (settlement.inbox != msg.sender) revert NotTheRecipient(settlement.inbox);
        if (settlement.reported) revert AlreadyReported();

        settlement.reported = true;
        emit SpamReported(messageId, msg.sender, settlement.payer);
    }

    /// @notice Withdraws the caller's accrued earnings to `to`.
    function claimEarnings(address to) external {
        if (to == address(0)) revert ZeroAddress();

        uint256 amount = earnings[msg.sender];
        if (amount == 0) revert NothingToClaim();
        // Zeroed and emitted before the external call below — same
        // ordering-as-reentrancy-guard reasoning as in payToSend.
        earnings[msg.sender] = 0;

        emit EarningsClaimed(msg.sender, to, amount);
        _pay(to, amount);
    }

    /// @notice The EIP-712 digest an enclave signs to quote a message. Exposed
    /// so off-chain signers and `payToSend`'s internal recovery compute the
    /// identical hash.
    function quoteDigest(
        bytes32 messageId,
        address inbox,
        Tier tier,
        uint256 amount,
        uint40 expiresAt
    ) public view returns (bytes32) {
        return _hashTypedDataV4(
            keccak256(abi.encode(QUOTE_TYPEHASH, messageId, inbox, uint8(tier), amount, expiresAt))
        );
    }

    function _recoverQuoteSigner(
        bytes32 messageId,
        address inbox,
        Tier tier,
        uint256 amount,
        uint40 expiresAt,
        bytes calldata signature
    ) private view returns (address) {
        return ECDSA.recover(quoteDigest(messageId, inbox, tier, amount, expiresAt), signature);
    }

    function _pay(address to, uint256 amount) private {
        (bool ok,) = to.call{value: amount}("");
        if (!ok) revert TransferFailed();
    }
}
