//! Contract addresses, chain parameters and ABI bindings for the Arc testnet
//! deployment. The ABIs are the exact JSON the web app ships in
//! `web/src/lib/contracts.ts`, so every function, event and error it declares
//! has a Rust binding; `sol!` derives selectors and topics from them.

use alloy_primitives::{Address, address};
use alloy_sol_types::sol;

pub use crate::format::USDC_DECIMALS;

sol!(
    #[sol(all_derives)]
    PostageEscrow,
    "abi/escrow.json"
);
sol!(
    #[sol(all_derives)]
    HumanRegistry,
    "abi/registry.json"
);
sol!(
    #[sol(all_derives)]
    PostageVault,
    "abi/vault.json"
);
sol!(
    #[sol(all_derives)]
    EnclaveRegistry,
    "abi/enclave_registry.json"
);

pub const POSTAGE_ESCROW: Address = address!("0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7");
pub const HUMAN_REGISTRY: Address = address!("0x0f9a1c7e971df81adc1b0335a527b30b6f136d05");
pub const POSTAGE_VAULT: Address = address!("0xd488a385529e9eec44a17b686f3b9372071f22dc");
pub const ENCLAVE_REGISTRY: Address = address!("0xf6afced17443571c79036f9d543ddbc2c5a645f9");

/// Static description of the chain the contracts live on (viem's `arcTestnet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chain {
    pub id: u64,
    pub name: &'static str,
    pub native_currency: NativeCurrency,
    /// Default HTTP RPC endpoints, in viem's order; the first is the primary.
    pub rpc_urls: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeCurrency {
    pub name: &'static str,
    pub symbol: &'static str,
    pub decimals: u32,
}

pub const ARC_TESTNET: Chain = Chain {
    id: 5_042_002,
    name: "Arc Testnet",
    native_currency: NativeCurrency {
        name: "USDC",
        symbol: "USDC",
        decimals: USDC_DECIMALS,
    },
    rpc_urls: &[
        "https://rpc.testnet.arc.network",
        "https://rpc.quicknode.testnet.arc.network",
        "https://rpc.blockdaemon.testnet.arc.network",
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, hex};
    use alloy_sol_types::{SolCall, SolError, SolEvent};

    #[test]
    fn addresses_match_the_web_constants() {
        let expected = [
            (POSTAGE_ESCROW, "0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7"),
            (HUMAN_REGISTRY, "0x0f9a1c7e971df81adc1b0335a527b30b6f136d05"),
            (POSTAGE_VAULT, "0xd488a385529e9eec44a17b686f3b9372071f22dc"),
            (
                ENCLAVE_REGISTRY,
                "0xf6afced17443571c79036f9d543ddbc2c5a645f9",
            ),
        ];
        for (actual, literal) in expected {
            assert_eq!(actual, literal.parse::<Address>().unwrap());
        }
    }

    #[test]
    fn chain_matches_viem_arc_testnet() {
        assert_eq!(ARC_TESTNET.id, 5_042_002);
        assert_eq!(ARC_TESTNET.name, "Arc Testnet");
        assert_eq!(ARC_TESTNET.native_currency.symbol, "USDC");
        assert_eq!(ARC_TESTNET.native_currency.decimals, 18);
        assert_eq!(ARC_TESTNET.rpc_urls[0], "https://rpc.testnet.arc.network");
        assert_eq!(ARC_TESTNET.rpc_urls.len(), 3);
    }

    #[test]
    fn escrow_function_selectors_match_the_web_abi() {
        assert_eq!(
            PostageEscrow::DEFAULT_FLOORCall::SELECTOR,
            hex!("3990483b"),
            "DEFAULT_FLOOR"
        );
        assert_eq!(
            PostageEscrow::VAULT_BPSCall::SELECTOR,
            hex!("a5424df3"),
            "VAULT_BPS"
        );
        assert_eq!(
            PostageEscrow::claimEarningsCall::SELECTOR,
            hex!("9ab891ba"),
            "claimEarnings"
        );
        assert_eq!(
            PostageEscrow::earningsCall::SELECTOR,
            hex!("543fd313"),
            "earnings"
        );
        assert_eq!(
            PostageEscrow::effectiveFloorCall::SELECTOR,
            hex!("552c804e"),
            "effectiveFloor"
        );
        assert_eq!(
            PostageEscrow::eip712DomainCall::SELECTOR,
            hex!("84b0196e"),
            "eip712Domain"
        );
        assert_eq!(
            PostageEscrow::floorPriceCall::SELECTOR,
            hex!("2aad9987"),
            "floorPrice"
        );
        assert_eq!(
            PostageEscrow::payToSendCall::SELECTOR,
            hex!("b6ba947b"),
            "payToSend"
        );
        assert_eq!(
            PostageEscrow::quoteDigestCall::SELECTOR,
            hex!("061d61e0"),
            "quoteDigest"
        );
        assert_eq!(
            PostageEscrow::registryCall::SELECTOR,
            hex!("7b103999"),
            "registry"
        );
        assert_eq!(
            PostageEscrow::reportSpamCall::SELECTOR,
            hex!("2f2f7dfc"),
            "reportSpam"
        );
        assert_eq!(
            PostageEscrow::setFloorPriceCall::SELECTOR,
            hex!("3d05829d"),
            "setFloorPrice"
        );
        assert_eq!(
            PostageEscrow::settledCall::SELECTOR,
            hex!("d945af1d"),
            "settled"
        );
        assert_eq!(
            PostageEscrow::settlementOfCall::SELECTOR,
            hex!("f1d3d381"),
            "settlementOf"
        );
        assert_eq!(
            PostageEscrow::vaultCall::SELECTOR,
            hex!("fbfa77cf"),
            "vault"
        );
    }

    #[test]
    fn escrow_event_topics_match_the_web_abi() {
        assert_eq!(
            PostageEscrow::EIP712DomainChanged::SIGNATURE_HASH,
            b256!("0a6387c9ea3628b88a633bb4f3b151770f70085117a15f9bf3787cda53f13d31"),
            "EIP712DomainChanged"
        );
        assert_eq!(
            PostageEscrow::EarningsClaimed::SIGNATURE_HASH,
            b256!("4ff12c7282f45b2ce31e461c62088156c528b57e12c159d55d57162d42222318"),
            "EarningsClaimed"
        );
        assert_eq!(
            PostageEscrow::FloorPriceSet::SIGNATURE_HASH,
            b256!("0b502d742a23ea1fc4516beeae6b314e217a4d75fc7e8f39041828cd4e00fd07"),
            "FloorPriceSet"
        );
        assert_eq!(
            PostageEscrow::Paid::SIGNATURE_HASH,
            b256!("402f33fd4c3150b6a6d300332596814a451f4281e40221c4e92cc12f7fdb577e"),
            "Paid"
        );
        assert_eq!(
            PostageEscrow::SpamReported::SIGNATURE_HASH,
            b256!("8ef74ae5638bec6bcf1f008d53c0f72112bac6ae51a68c88e6941f20176294c7"),
            "SpamReported"
        );
    }

    #[test]
    fn escrow_error_selectors_match_the_web_abi() {
        assert_eq!(
            PostageEscrow::AlreadyReported::SELECTOR,
            hex!("770c15e3"),
            "AlreadyReported"
        );
        assert_eq!(
            PostageEscrow::AlreadySettled::SELECTOR,
            hex!("560ff900"),
            "AlreadySettled"
        );
        assert_eq!(
            PostageEscrow::BelowFloor::SELECTOR,
            hex!("dc6ef75d"),
            "BelowFloor"
        );
        assert_eq!(
            PostageEscrow::ECDSAInvalidSignature::SELECTOR,
            hex!("f645eedf"),
            "ECDSAInvalidSignature"
        );
        assert_eq!(
            PostageEscrow::ECDSAInvalidSignatureLength::SELECTOR,
            hex!("fce698f7"),
            "ECDSAInvalidSignatureLength"
        );
        assert_eq!(
            PostageEscrow::ECDSAInvalidSignatureS::SELECTOR,
            hex!("d78bce0c"),
            "ECDSAInvalidSignatureS"
        );
        assert_eq!(
            PostageEscrow::InvalidShortString::SELECTOR,
            hex!("b3512b0c"),
            "InvalidShortString"
        );
        assert_eq!(
            PostageEscrow::NotSettled::SELECTOR,
            hex!("ba329a9b"),
            "NotSettled"
        );
        assert_eq!(
            PostageEscrow::NotTheRecipient::SELECTOR,
            hex!("02a6c052"),
            "NotTheRecipient"
        );
        assert_eq!(
            PostageEscrow::NothingToClaim::SELECTOR,
            hex!("969bf728"),
            "NothingToClaim"
        );
        assert_eq!(
            PostageEscrow::QuoteExpired::SELECTOR,
            hex!("8727a7f9"),
            "QuoteExpired"
        );
        assert_eq!(
            PostageEscrow::StringTooLong::SELECTOR,
            hex!("305a27a9"),
            "StringTooLong"
        );
        assert_eq!(
            PostageEscrow::TransferFailed::SELECTOR,
            hex!("90b8ec18"),
            "TransferFailed"
        );
        assert_eq!(
            PostageEscrow::Underpaid::SELECTOR,
            hex!("f3ebc384"),
            "Underpaid"
        );
        assert_eq!(
            PostageEscrow::UnknownEnclave::SELECTOR,
            hex!("ac88b767"),
            "UnknownEnclave"
        );
        assert_eq!(
            PostageEscrow::ZeroAddress::SELECTOR,
            hex!("d92e233d"),
            "ZeroAddress"
        );
    }

    #[test]
    fn registry_function_selectors_match_the_web_abi() {
        assert_eq!(
            HumanRegistry::attestCall::SELECTOR,
            hex!("92c68a7b"),
            "attest"
        );
        assert_eq!(
            HumanRegistry::attesterCall::SELECTOR,
            hex!("47b0c3b3"),
            "attester"
        );
        assert_eq!(
            HumanRegistry::domainSeparatorCall::SELECTOR,
            hex!("f698da25"),
            "domainSeparator"
        );
        assert_eq!(
            HumanRegistry::eip712DomainCall::SELECTOR,
            hex!("84b0196e"),
            "eip712Domain"
        );
        assert_eq!(
            HumanRegistry::humanUntilCall::SELECTOR,
            hex!("754e5859"),
            "humanUntil"
        );
        assert_eq!(
            HumanRegistry::isHumanCall::SELECTOR,
            hex!("f72c436f"),
            "isHuman"
        );
        assert_eq!(
            HumanRegistry::nullifierOwnerCall::SELECTOR,
            hex!("189c387f"),
            "nullifierOwner"
        );
    }

    #[test]
    fn registry_event_topics_match_the_web_abi() {
        assert_eq!(
            HumanRegistry::EIP712DomainChanged::SIGNATURE_HASH,
            b256!("0a6387c9ea3628b88a633bb4f3b151770f70085117a15f9bf3787cda53f13d31"),
            "EIP712DomainChanged"
        );
        assert_eq!(
            HumanRegistry::HumanAttested::SIGNATURE_HASH,
            b256!("63737fe60ee733063ef95ccb1b46a65912714b15c645e845598f3b1efb1e8346"),
            "HumanAttested"
        );
    }

    #[test]
    fn registry_error_selectors_match_the_web_abi() {
        assert_eq!(
            HumanRegistry::AttestationExpired::SELECTOR,
            hex!("716dcc39"),
            "AttestationExpired"
        );
        assert_eq!(
            HumanRegistry::ECDSAInvalidSignature::SELECTOR,
            hex!("f645eedf"),
            "ECDSAInvalidSignature"
        );
        assert_eq!(
            HumanRegistry::ECDSAInvalidSignatureLength::SELECTOR,
            hex!("fce698f7"),
            "ECDSAInvalidSignatureLength"
        );
        assert_eq!(
            HumanRegistry::ECDSAInvalidSignatureS::SELECTOR,
            hex!("d78bce0c"),
            "ECDSAInvalidSignatureS"
        );
        assert_eq!(
            HumanRegistry::InvalidShortString::SELECTOR,
            hex!("b3512b0c"),
            "InvalidShortString"
        );
        assert_eq!(
            HumanRegistry::InvalidSignature::SELECTOR,
            hex!("8baa579f"),
            "InvalidSignature"
        );
        assert_eq!(
            HumanRegistry::NotAnExtension::SELECTOR,
            hex!("f515da8f"),
            "NotAnExtension"
        );
        assert_eq!(
            HumanRegistry::NullifierAlreadyBound::SELECTOR,
            hex!("9a825a9a"),
            "NullifierAlreadyBound"
        );
        assert_eq!(
            HumanRegistry::StringTooLong::SELECTOR,
            hex!("305a27a9"),
            "StringTooLong"
        );
    }

    #[test]
    fn vault_function_selectors_match_the_web_abi() {
        assert_eq!(
            PostageVault::TREASURY_BPSCall::SELECTOR,
            hex!("4c25d197"),
            "TREASURY_BPS"
        );
        assert_eq!(
            PostageVault::refillRelayerCall::SELECTOR,
            hex!("7fc914a8"),
            "refillRelayer"
        );
        assert_eq!(
            PostageVault::relayerCall::SELECTOR,
            hex!("8406c079"),
            "relayer"
        );
        assert_eq!(
            PostageVault::sponsorshipPoolCall::SELECTOR,
            hex!("fda39083"),
            "sponsorshipPool"
        );
        assert_eq!(
            PostageVault::treasuryCall::SELECTOR,
            hex!("61d027b3"),
            "treasury"
        );
        assert_eq!(
            PostageVault::treasuryBalanceCall::SELECTOR,
            hex!("313dab20"),
            "treasuryBalance"
        );
        assert_eq!(
            PostageVault::withdrawTreasuryCall::SELECTOR,
            hex!("0d86419a"),
            "withdrawTreasury"
        );
    }

    #[test]
    fn vault_event_topics_match_the_web_abi() {
        assert_eq!(
            PostageVault::Funded::SIGNATURE_HASH,
            b256!("77360216dafe21aa8d455333608207082f74e837ba9b3ef15d0a73cd22693738"),
            "Funded"
        );
        assert_eq!(
            PostageVault::RelayerRefilled::SIGNATURE_HASH,
            b256!("c53213d0690849956315de78e333ccf3b581cbfb2699375d87b253648e225f60"),
            "RelayerRefilled"
        );
        assert_eq!(
            PostageVault::TreasuryWithdrawn::SIGNATURE_HASH,
            b256!("41fdd680478135993bc53fb2ffaf9560951b57ef62ff6badd02b61e018b4f17f"),
            "TreasuryWithdrawn"
        );
    }

    #[test]
    fn vault_error_selectors_match_the_web_abi() {
        assert_eq!(
            PostageVault::InsufficientPool::SELECTOR,
            hex!("b9b3b6b9"),
            "InsufficientPool"
        );
        assert_eq!(
            PostageVault::NotTreasury::SELECTOR,
            hex!("b90cdbb1"),
            "NotTreasury"
        );
        assert_eq!(
            PostageVault::TransferFailed::SELECTOR,
            hex!("90b8ec18"),
            "TransferFailed"
        );
        assert_eq!(
            PostageVault::ZeroAddress::SELECTOR,
            hex!("d92e233d"),
            "ZeroAddress"
        );
    }

    #[test]
    fn enclave_registry_function_selectors_match_the_web_abi() {
        assert_eq!(
            EnclaveRegistry::expectedMeasurementCall::SELECTOR,
            hex!("9af33e78"),
            "expectedMeasurement"
        );
        assert_eq!(
            EnclaveRegistry::isRegisteredCall::SELECTOR,
            hex!("c3c5a547"),
            "isRegistered"
        );
        assert_eq!(
            EnclaveRegistry::measurementOfCall::SELECTOR,
            hex!("1b727ddd"),
            "measurementOf"
        );
        assert_eq!(
            EnclaveRegistry::ownerCall::SELECTOR,
            hex!("8da5cb5b"),
            "owner"
        );
        assert_eq!(
            EnclaveRegistry::registerCall::SELECTOR,
            hex!("4420e486"),
            "register"
        );
        assert_eq!(
            EnclaveRegistry::revokeCall::SELECTOR,
            hex!("74a8f103"),
            "revoke"
        );
        assert_eq!(
            EnclaveRegistry::setMeasurementCall::SELECTOR,
            hex!("28875917"),
            "setMeasurement"
        );
    }

    #[test]
    fn enclave_registry_event_topics_match_the_web_abi() {
        assert_eq!(
            EnclaveRegistry::EnclaveRegistered::SIGNATURE_HASH,
            b256!("f461d2fcd0b737b85ec49b0a48e24b2de72534d256bf45886fe727cbc47b6b68"),
            "EnclaveRegistered"
        );
        assert_eq!(
            EnclaveRegistry::EnclaveRevoked::SIGNATURE_HASH,
            b256!("014328d04215ef25a11c630f007f31516a065906981503bcb8cdf4998651c18b"),
            "EnclaveRevoked"
        );
        assert_eq!(
            EnclaveRegistry::MeasurementSet::SIGNATURE_HASH,
            b256!("a8020ea53765eecef0b820553e15cf2618e6fc9495c19412015bdbdd822e625b"),
            "MeasurementSet"
        );
    }

    #[test]
    fn enclave_registry_error_selectors_match_the_web_abi() {
        assert_eq!(
            EnclaveRegistry::AlreadyRegistered::SELECTOR,
            hex!("3a81d6fc"),
            "AlreadyRegistered"
        );
        assert_eq!(
            EnclaveRegistry::NoMeasurementSet::SELECTOR,
            hex!("adbc50a0"),
            "NoMeasurementSet"
        );
        assert_eq!(
            EnclaveRegistry::NotOwner::SELECTOR,
            hex!("30cd7471"),
            "NotOwner"
        );
        assert_eq!(
            EnclaveRegistry::ZeroAddress::SELECTOR,
            hex!("d92e233d"),
            "ZeroAddress"
        );
    }
}
