// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {HumanRegistry} from "../src/HumanRegistry.sol";
import {PostageEscrow} from "../src/PostageEscrow.sol";
import {PostageVault} from "../src/PostageVault.sol";

contract Deploy is Script {
    function run() external {
        address attester = vm.envAddress("ATTESTER_ADDRESS");
        address treasury = vm.envAddress("TREASURY_ADDRESS");
        address relayer = vm.envAddress("RELAYER_ADDRESS");

        vm.startBroadcast();
        PostageVault vault = new PostageVault(treasury, relayer);
        PostageEscrow escrow = new PostageEscrow(address(vault));
        HumanRegistry registry = new HumanRegistry(attester);
        vm.stopBroadcast();

        console.log("chain id        ", block.chainid);
        console.log("PostageVault    ", address(vault));
        console.log("PostageEscrow   ", address(escrow));
        console.log("HumanRegistry   ", address(registry));
        console.log("attester        ", attester);
        console.log("treasury        ", treasury);
        console.log("relayer         ", relayer);
    }
}
