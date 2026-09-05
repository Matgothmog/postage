// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/// @notice Holds postage that a stranger attaches to a message until the
/// recipient decides whether it was worth receiving.
///
/// Postage is paid in native USDC, which on Arc is the gas token, so a stamp
/// and the transaction that carries it are both denominated in dollars.
/// Amounts here use the native 18-decimal representation, never the 6-decimal
/// ERC-20 view of the same balance.
contract PostageEscrow {
    enum Status {
        None,
        Held,
        Released,
        Claimed,
        Expired
    }

    struct Stamp {
        address sender;
        address recipient;
        uint256 amount;
        uint64 postedAt;
        Status status;
    }

    uint64 public constant EXPIRY = 14 days;

    /// @notice Minimum postage each inbox owner requires from a stranger.
    mapping(address inbox => uint256 amount) public price;

    mapping(bytes32 messageId => Stamp) public stamps;

    event PriceSet(address indexed inbox, uint256 amount);
    event StampPosted(
        bytes32 indexed messageId,
        address indexed sender,
        address indexed recipient,
        uint256 amount
    );
    event StampReleased(bytes32 indexed messageId, address indexed sender, uint256 amount);
    event StampClaimed(bytes32 indexed messageId, address indexed recipient, uint256 amount);
    event StampExpired(bytes32 indexed messageId, address indexed sender, uint256 amount);

    error StampAlreadyExists();
    error StampNotHeld();
    error NotRecipient();
    error NotYetExpired();
    error InvalidRecipient();
    error PostageTooLow(uint256 required, uint256 provided);
    error TransferFailed();

    function setPrice(uint256 amount) external {
        price[msg.sender] = amount;
        emit PriceSet(msg.sender, amount);
    }

    /// @param messageId Hash of the message this stamp pays for.
    function postStamp(bytes32 messageId, address recipient) external payable {
        if (recipient == address(0)) revert InvalidRecipient();
        if (stamps[messageId].status != Status.None) revert StampAlreadyExists();

        uint256 required = price[recipient];
        if (msg.value < required) revert PostageTooLow(required, msg.value);

        stamps[messageId] = Stamp({
            sender: msg.sender,
            recipient: recipient,
            amount: msg.value,
            postedAt: uint64(block.timestamp),
            status: Status.Held
        });

        emit StampPosted(messageId, msg.sender, recipient, msg.value);
    }

    /// @notice The message was legitimate. Give the postage back.
    function release(bytes32 messageId) external {
        Stamp storage stamp = _heldStamp(messageId);
        if (msg.sender != stamp.recipient) revert NotRecipient();

        (address sender, uint256 amount) = (stamp.sender, stamp.amount);
        stamp.status = Status.Released;

        emit StampReleased(messageId, sender, amount);
        _pay(sender, amount);
    }

    /// @notice The message was spam. Keep the postage.
    function claim(bytes32 messageId) external {
        Stamp storage stamp = _heldStamp(messageId);
        if (msg.sender != stamp.recipient) revert NotRecipient();

        uint256 amount = stamp.amount;
        stamp.status = Status.Claimed;

        emit StampClaimed(messageId, msg.sender, amount);
        _pay(msg.sender, amount);
    }

    /// @notice Recipients who never respond do not get to hold postage forever.
    function expire(bytes32 messageId) external {
        Stamp storage stamp = _heldStamp(messageId);
        if (block.timestamp < stamp.postedAt + EXPIRY) revert NotYetExpired();

        (address sender, uint256 amount) = (stamp.sender, stamp.amount);
        stamp.status = Status.Expired;

        emit StampExpired(messageId, sender, amount);
        _pay(sender, amount);
    }

    function _heldStamp(bytes32 messageId) private view returns (Stamp storage stamp) {
        stamp = stamps[messageId];
        if (stamp.status != Status.Held) revert StampNotHeld();
    }

    function _pay(address to, uint256 amount) private {
        (bool ok,) = to.call{value: amount}("");
        if (!ok) revert TransferFailed();
    }
}
