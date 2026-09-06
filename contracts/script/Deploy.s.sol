// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {EnclaveRegistry} from "../src/EnclaveRegistry.sol";
import {HumanRegistry} from "../src/HumanRegistry.sol";
import {PostageEscrow} from "../src/PostageEscrow.sol";
import {PostageVault} from "../src/PostageVault.sol";

contract Deploy is Script {
    function run() external {
        address attester = vm.envAddress("ATTESTER_ADDRESS");
        address treasury = vm.envAddress("TREASURY_ADDRESS");
        address relayer = vm.envAddress("RELAYER_ADDRESS");
        address owner = vm.envAddress("DEPLOYER_ADDRESS");

        vm.startBroadcast();
        PostageVault vault = new PostageVault(treasury, relayer);
        EnclaveRegistry enclaves = new EnclaveRegistry(owner);
        PostageEscrow escrow = new PostageEscrow(address(enclaves), address(vault));
        HumanRegistry humans = new HumanRegistry(attester);
        vm.stopBroadcast();

        console.log("chain id        ", block.chainid);
        console.log("PostageVault    ", address(vault));
        console.log("EnclaveRegistry ", address(enclaves));
        console.log("PostageEscrow   ", address(escrow));
        console.log("HumanRegistry   ", address(humans));
    }
}
