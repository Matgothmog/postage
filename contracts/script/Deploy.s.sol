// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {HumanRegistry} from "../src/HumanRegistry.sol";
import {PostageEscrow} from "../src/PostageEscrow.sol";

contract Deploy is Script {
    function run() external {
        address attester = vm.envAddress("ATTESTER_ADDRESS");

        vm.startBroadcast();
        PostageEscrow escrow = new PostageEscrow();
        HumanRegistry registry = new HumanRegistry(attester);
        vm.stopBroadcast();

        console.log("chain id        ", block.chainid);
        console.log("PostageEscrow   ", address(escrow));
        console.log("HumanRegistry   ", address(registry));
        console.log("attester        ", attester);
    }
}
