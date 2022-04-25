use sbor::DecodeError;
use scrypto::buffer::scrypto_decode;
use scrypto::core::SystemFunction;
use scrypto::values::ScryptoValue;
use crate::engine::SystemApi;

#[derive(Debug, Clone, PartialEq)]
pub enum SystemError {
    InvalidRequestData(DecodeError),
}

pub struct System {}

impl System {
    pub fn static_main<S: SystemApi>(
        arg: ScryptoValue,
        system_api: &mut S,
    ) -> Result<ScryptoValue, SystemError> {
        let function: SystemFunction = scrypto_decode(&arg.raw).map_err(|e| SystemError::InvalidRequestData(e))?;
        match function {
            SystemFunction::GetEpoch() => {
                // TODO: Make this stateful
                Ok(ScryptoValue::from_value(&system_api.get_epoch()))
            }
            SystemFunction::GetTransactionHash() => {
                Ok(ScryptoValue::from_value(&system_api.get_transaction_hash()))
            }
        }
    }
}