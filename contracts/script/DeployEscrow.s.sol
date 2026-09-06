// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {PostageEscrow} from "../src/PostageEscrow.sol";

/// @notice Replaces only the escrow, against the registry and vault already
/// deployed.
///
/// Deploy.s.sol builds the whole system from nothing, which is the wrong tool
/// once anything is live: it would orphan the signing key registered in
/// EnclaveRegistry and the balance held by PostageVault. The escrow is the only
/// contract that holds no state worth keeping, because earnings are claimed to
/// the inbox owner rather than accumulated.
contract DeployEscrow is Script {
    function run() external {
        address registry = vm.envAddress("ENCLAVE_REGISTRY_ADDRESS");
        address vault = vm.envAddress("POSTAGE_VAULT_ADDRESS");

        vm.startBroadcast();
        PostageEscrow escrow = new PostageEscrow(registry, vault);
        vm.stopBroadcast();

        console.log("chain id      ", block.chainid);
        console.log("PostageEscrow ", address(escrow));
        console.log("registry      ", address(escrow.registry()));
        console.log("vault         ", escrow.vault());
        console.log("default floor ", escrow.DEFAULT_FLOOR());
    }
}
