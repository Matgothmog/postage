// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {EnclaveRegistry} from "../src/EnclaveRegistry.sol";
import {HumanRegistry} from "../src/HumanRegistry.sol";
import {PostageEscrow} from "../src/PostageEscrow.sol";
import {PostageVault} from "../src/PostageVault.sol";

/// @notice Deploys the whole system from nothing.
///
/// Do not run this against Arc testnet as of this writing: all four contracts
/// are already live there, holding state a redeploy would orphan — the one
/// registered signing key in EnclaveRegistry, PostageVault's treasury and
/// sponsorship balances, and every attestation in HumanRegistry. Deploying
/// fresh contracts here does not migrate any of that; it abandons it behind
/// addresses nothing points at anymore. If only the escrow needs to be
/// replaced, use `DeployEscrow.s.sol` instead, which redeploys against the
/// registry and vault already live and explains why in its own header.
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
