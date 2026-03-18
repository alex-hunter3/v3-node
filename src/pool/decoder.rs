//! Uniswap V3-style pool calldata decoder.

use alloy::primitives::Bytes;

/// Decoded pool function call.
#[derive(Debug)]
pub enum PoolCall {
    Swap {
        zero_for_one: bool,
        amount_specified: i128,
    },
    Mint,
    Burn,
    Collect,
    Unknown(u32), // selector we don't recognise
}

// Uniswap V3 function selectors
const SEL_SWAP: u32 = 0x128acb08;
const SEL_MINT: u32 = 0x3c8a7d8d;
const SEL_BURN: u32 = 0xa34123a7;
const SEL_COLLECT: u32 = 0x4f1eb3d8;

pub fn decode_pool_calldata(input: &Bytes) -> Option<PoolCall> {
    if input.len() < 4 {
        return None;
    }

    let selector = u32::from_be_bytes(input[..4].try_into().ok()?);

    match selector {
        SEL_SWAP => {
            // swap(address,bool,int256,uint160,bytes)
            // bool zeroForOne is at offset 4 + 32 = 36 (after address param)
            if input.len() < 68 {
                return Some(PoolCall::Swap {
                    zero_for_one: false,
                    amount_specified: 0,
                });
            }
            let zero_for_one = input[35] != 0;
            // int256 amountSpecified at offset 68
            let amount_bytes: [u8; 16] = input[52..68].try_into().ok()?;
            let amount_specified = i128::from_be_bytes(amount_bytes);
            Some(PoolCall::Swap {
                zero_for_one,
                amount_specified,
            })
        }
        SEL_MINT => Some(PoolCall::Mint),
        SEL_BURN => Some(PoolCall::Burn),
        SEL_COLLECT => Some(PoolCall::Collect),
        other => Some(PoolCall::Unknown(other)),
    }
}
