use sbor::rust::borrow::ToOwned;
use sbor::rust::collections::BTreeSet;
use sbor::rust::fmt;
use sbor::rust::fmt::Debug;
use sbor::rust::str::FromStr;
use sbor::rust::string::String;
use sbor::rust::vec::Vec;
use sbor::*;
use utils::copy_u8_array;

use crate::abi::*;
use crate::api::{api::*, types::*};
use crate::data::ScryptoCustomTypeId;
use crate::math::*;
use crate::scrypto;
use crate::scrypto_type;
use crate::wasm::*;

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultPutInvocation {
    pub receiver: VaultId,
    pub bucket: Bucket,
}

impl Invocation for VaultPutInvocation {
    type Output = ();
}

impl ScryptoNativeInvocation for VaultPutInvocation {
    type ScryptoOutput = ();
}

impl Into<NativeFnInvocation> for VaultPutInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(VaultMethodInvocation::Put(
            self,
        )))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultTakeInvocation {
    pub receiver: VaultId,
    pub amount: Decimal,
}

impl Invocation for VaultTakeInvocation {
    type Output = Bucket;
}

impl ScryptoNativeInvocation for VaultTakeInvocation {
    type ScryptoOutput = Bucket;
}

impl Into<NativeFnInvocation> for VaultTakeInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(VaultMethodInvocation::Take(
            self,
        )))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultTakeNonFungiblesInvocation {
    pub receiver: VaultId,
    pub non_fungible_ids: BTreeSet<NonFungibleId>,
}

impl Invocation for VaultTakeNonFungiblesInvocation {
    type Output = Bucket;
}

impl ScryptoNativeInvocation for VaultTakeNonFungiblesInvocation {
    type ScryptoOutput = Bucket;
}

impl Into<NativeFnInvocation> for VaultTakeNonFungiblesInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(
            VaultMethodInvocation::TakeNonFungibles(self),
        ))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultGetAmountInvocation {
    pub receiver: VaultId,
}

impl Invocation for VaultGetAmountInvocation {
    type Output = Decimal;
}

impl ScryptoNativeInvocation for VaultGetAmountInvocation {
    type ScryptoOutput = Decimal;
}

impl Into<NativeFnInvocation> for VaultGetAmountInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(
            VaultMethodInvocation::GetAmount(self),
        ))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultGetResourceAddressInvocation {
    pub receiver: VaultId,
}

impl Invocation for VaultGetResourceAddressInvocation {
    type Output = ResourceAddress;
}

impl ScryptoNativeInvocation for VaultGetResourceAddressInvocation {
    type ScryptoOutput = ResourceAddress;
}

impl Into<NativeFnInvocation> for VaultGetResourceAddressInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(
            VaultMethodInvocation::GetResourceAddress(self),
        ))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultGetNonFungibleIdsInvocation {
    pub receiver: VaultId,
}

impl Invocation for VaultGetNonFungibleIdsInvocation {
    type Output = BTreeSet<NonFungibleId>;
}

impl ScryptoNativeInvocation for VaultGetNonFungibleIdsInvocation {
    type ScryptoOutput = BTreeSet<NonFungibleId>;
}

impl Into<NativeFnInvocation> for VaultGetNonFungibleIdsInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(
            VaultMethodInvocation::GetNonFungibleIds(self),
        ))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultCreateProofInvocation {
    pub receiver: VaultId,
}

impl Invocation for VaultCreateProofInvocation {
    type Output = Proof;
}

impl ScryptoNativeInvocation for VaultCreateProofInvocation {
    type ScryptoOutput = Proof;
}

impl Into<NativeFnInvocation> for VaultCreateProofInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(
            VaultMethodInvocation::CreateProof(self),
        ))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultCreateProofByAmountInvocation {
    pub receiver: VaultId,
    pub amount: Decimal,
}

impl Invocation for VaultCreateProofByAmountInvocation {
    type Output = Proof;
}

impl ScryptoNativeInvocation for VaultCreateProofByAmountInvocation {
    type ScryptoOutput = Proof;
}

impl Into<NativeFnInvocation> for VaultCreateProofByAmountInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(
            VaultMethodInvocation::CreateProofByAmount(self),
        ))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultCreateProofByIdsInvocation {
    pub receiver: VaultId,
    pub ids: BTreeSet<NonFungibleId>,
}

impl Invocation for VaultCreateProofByIdsInvocation {
    type Output = Proof;
}

impl ScryptoNativeInvocation for VaultCreateProofByIdsInvocation {
    type ScryptoOutput = Proof;
}

impl Into<NativeFnInvocation> for VaultCreateProofByIdsInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(
            VaultMethodInvocation::CreateProofByIds(self),
        ))
    }
}

#[derive(Debug)]
#[scrypto(TypeId, Encode, Decode)]
pub struct VaultLockFeeInvocation {
    pub receiver: VaultId,
    pub amount: Decimal,
    pub contingent: bool,
}

impl Invocation for VaultLockFeeInvocation {
    type Output = ();
}

impl ScryptoNativeInvocation for VaultLockFeeInvocation {
    type ScryptoOutput = ();
}

impl Into<NativeFnInvocation> for VaultLockFeeInvocation {
    fn into(self) -> NativeFnInvocation {
        NativeFnInvocation::Method(NativeMethodInvocation::Vault(
            VaultMethodInvocation::LockFee(self),
        ))
    }
}

#[derive(PartialEq, Eq, Hash)]
pub struct Vault(pub VaultId);

//========
// error
//========

/// Represents an error when decoding vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseVaultError {
    InvalidHex(String),
    InvalidLength(usize),
}

#[cfg(not(feature = "alloc"))]
impl std::error::Error for ParseVaultError {}

#[cfg(not(feature = "alloc"))]
impl fmt::Display for ParseVaultError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

//========
// binary
//========

impl TryFrom<&[u8]> for Vault {
    type Error = ParseVaultError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        match slice.len() {
            36 => Ok(Self(copy_u8_array(slice))),
            _ => Err(ParseVaultError::InvalidLength(slice.len())),
        }
    }
}

impl Vault {
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

scrypto_type!(Vault, ScryptoCustomTypeId::Vault, Type::Vault, 36);

//======
// text
//======

impl FromStr for Vault {
    type Err = ParseVaultError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s).map_err(|_| ParseVaultError::InvalidHex(s.to_owned()))?;
        Self::try_from(bytes.as_slice())
    }
}

impl fmt::Display for Vault {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", hex::encode(self.to_vec()))
    }
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self)
    }
}