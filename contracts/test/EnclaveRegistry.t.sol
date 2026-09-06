// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {EnclaveRegistry} from "../src/EnclaveRegistry.sol";

contract EnclaveRegistryTest is Test {
    EnclaveRegistry internal registry;

    address internal owner = makeAddr("owner");
    address internal anyone = makeAddr("anyone");
    address internal enclave = makeAddr("enclave");

    bytes32 internal constant MEASUREMENT = keccak256("pcr0");

    function setUp() public {
        registry = new EnclaveRegistry(owner);
    }

    function test_registrationRecordsTheMeasurementItWasAllowedUnder() public {
        vm.startPrank(owner);
        registry.setMeasurement(MEASUREMENT);
        registry.register(enclave);
        vm.stopPrank();

        assertTrue(registry.isRegistered(enclave));
        assertEq(registry.measurementOf(enclave), MEASUREMENT);
    }

    /// A key cannot be trusted before anyone has said which image may sign,
    /// otherwise the registry would vouch for code nobody can identify.
    function test_cannotRegisterBeforeAMeasurementExists() public {
        vm.prank(owner);
        vm.expectRevert(EnclaveRegistry.NoMeasurementSet.selector);
        registry.register(enclave);
    }

    function test_onlyOwnerRegisters() public {
        vm.prank(owner);
        registry.setMeasurement(MEASUREMENT);

        vm.prank(anyone);
        vm.expectRevert(EnclaveRegistry.NotOwner.selector);
        registry.register(enclave);
    }

    function test_onlyOwnerSetsTheMeasurement() public {
        vm.prank(anyone);
        vm.expectRevert(EnclaveRegistry.NotOwner.selector);
        registry.setMeasurement(MEASUREMENT);
    }

    function test_revokedKeyStopsBeingAccepted() public {
        vm.startPrank(owner);
        registry.setMeasurement(MEASUREMENT);
        registry.register(enclave);
        registry.revoke(enclave);
        vm.stopPrank();

        assertFalse(registry.isRegistered(enclave));
    }

    /// Rotating the measurement must not retroactively bless a key registered
    /// under an older image.
    function test_rotatingTheMeasurementLeavesOldRegistrationsTraceable() public {
        vm.startPrank(owner);
        registry.setMeasurement(MEASUREMENT);
        registry.register(enclave);

        bytes32 newMeasurement = keccak256("pcr0-v2");
        registry.setMeasurement(newMeasurement);
        vm.stopPrank();

        assertEq(registry.measurementOf(enclave), MEASUREMENT, "still records what it signed under");
        assertEq(registry.expectedMeasurement(), newMeasurement);
    }

    function test_rejectsZeroAddresses() public {
        vm.expectRevert(EnclaveRegistry.ZeroAddress.selector);
        new EnclaveRegistry(address(0));

        vm.startPrank(owner);
        registry.setMeasurement(MEASUREMENT);
        vm.expectRevert(EnclaveRegistry.ZeroAddress.selector);
        registry.register(address(0));
        vm.stopPrank();
    }

    function test_doubleRegistrationIsRejected() public {
        vm.startPrank(owner);
        registry.setMeasurement(MEASUREMENT);
        registry.register(enclave);
        vm.expectRevert(EnclaveRegistry.AlreadyRegistered.selector);
        registry.register(enclave);
        vm.stopPrank();
    }
}
