use p256::ecdsa::{signature::Signer, SigningKey};
use p256::SecretKey;
use sbor::rust::vec::Vec;

use scrypto::crypto::*;

pub struct EcdsaPrivateKey(SecretKey);

impl EcdsaPrivateKey {
    /* all public methods are confined to this impl */

    pub const LENGTH: usize = 32;

    pub fn public_key(&self) -> EcdsaPublicKey {
        EcdsaPublicKey(self.0.public_key())
    }

    pub fn sign(&self, msg: &[u8]) -> EcdsaSignature {
        let signer = SigningKey::from(&self.0);
        EcdsaSignature(signer.sign(msg))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.to_be_bytes().as_slice().to_vec()
    }

    pub fn from_bytes(slice: &[u8]) -> Result<Self, ()> {
        if slice.len() != EcdsaPrivateKey::LENGTH {
            return Err(());
        }
        Ok(Self(SecretKey::from_be_bytes(slice).map_err(|_| ())?))
    }

    pub fn from_u64(n: u64) -> Result<Self, ()> {
        let mut bytes = [0u8; EcdsaPrivateKey::LENGTH];
        (&mut bytes[EcdsaPrivateKey::LENGTH - 8..EcdsaPrivateKey::LENGTH])
            .copy_from_slice(&n.to_be_bytes());

        Ok(Self(SecretKey::from_be_bytes(&bytes).map_err(|_| ())?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbor::rust::str::FromStr;
    use scrypto::crypto::Hash;

    #[test]
    fn sign_and_verify() {
        // From Babylon Wallet PoC
        let test_sk = "0000000000000000000000000000000000000000000000000000000000000001";
        let test_pk = "046b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c2964fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5";
        let test_message = "{\"a\":\"banan\"}";
        let test_hash = "c43a1e3a7e822c97004267324ba8df88d114ab3e019d0e85eccb1ff8592d6d36";
        let test_signature = "468764c570758020eb8392e40de5805757d6e563a507f12ddde56463c23820e10401cae1684cb350bc3ecb45965ee259964f931eb4c165cd1a270fc538b65a75";
        let sk = EcdsaPrivateKey::from_bytes(&hex::decode(test_sk).unwrap()).unwrap();
        let pk = EcdsaPublicKey::from_str(test_pk).unwrap();
        let hash = Hash::from_str(test_hash).unwrap();
        let sig = EcdsaSignature::from_str(test_signature).unwrap();

        assert_eq!(sk.public_key(), pk);
        assert_eq!(scrypto::crypto::hash(test_message), hash);
        assert_eq!(sk.sign(test_message.as_bytes()), sig);
        assert!(EcdsaVerifier::verify(test_message.as_bytes(), &pk, &sig));
    }
}
