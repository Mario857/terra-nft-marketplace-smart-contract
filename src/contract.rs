use std::cmp::Ordering;
use std::str::from_utf8;

#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    from_binary, to_binary, Api, Binary, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Order, Pair,
    Response, StdError, StdResult, Uint128, WasmMsg,
};
use cw2::set_contract_version;
use cw20::{Cw20ExecuteMsg, Cw20ReceiveMsg};
use cw721::{Cw721ExecuteMsg, Cw721ReceiveMsg};

use crate::error::ContractError;
use crate::msg::{BuyNft, ExecuteMsg, InstantiateMsg, QueryMsg, SellNft};
use crate::package::{ContractInfoResponse, OfferingsResponse, Paged, QueryOfferingsResult};
use crate::state::{increment_offerings, Offering, CONTRACT_INFO, OFFERINGS};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:cw-marketplace";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    let info = ContractInfoResponse { name: msg.name };

    CONTRACT_INFO.save(deps.storage, &info)?;

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new().add_attribute("method", "instantiate"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::WithdrawNft { offering_id } => try_withdraw(deps, env, info, offering_id),
        ExecuteMsg::Receive(msg) => try_receive(deps, env, info, msg),
        ExecuteMsg::ReceiveNft(msg) => try_receive_nft(deps, env, info, msg),
    }
}

pub fn try_receive(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    rcv_msg: Cw20ReceiveMsg,
) -> Result<Response, ContractError> {
    let msg: BuyNft = from_binary(&rcv_msg.msg)?;

    let off = OFFERINGS.load(deps.storage, &msg.offering_id)?;

    if rcv_msg.amount < off.list_price.amount {
        return Err(ContractError::InsufficientFunds {});
    }

    // check for enough coins
    let transfer_cw20_msg = Cw20ExecuteMsg::Transfer {
        recipient: deps.api.addr_humanize(&off.seller)?.into_string(),
        amount: rcv_msg.amount,
    };

    // create transfer cw20 msg
    let exec_cw20_transfer = WasmMsg::Execute {
        contract_addr: info.sender.clone().into_string(),
        msg: to_binary(&transfer_cw20_msg)?,
        funds: vec![],
    };

    // create transfer cw721 msg
    let transfer_cw721_msg = Cw721ExecuteMsg::TransferNft {
        recipient: rcv_msg.sender.clone(),
        token_id: off.token_id.clone(),
    };

    let exec_cw721_transfer = WasmMsg::Execute {
        contract_addr: deps.api.addr_humanize(&off.contract_addr)?.to_string(),
        msg: to_binary(&transfer_cw721_msg)?,
        funds: vec![],
    };

    // if everything is fine transfer cw20 to seller
    let cw20_transfer_cosmos_msg: CosmosMsg = exec_cw20_transfer.into();
    // transfer nft to buyer
    let cw721_transfer_cosmos_msg: CosmosMsg = exec_cw721_transfer.into();

    let cosmos_msgs = vec![cw20_transfer_cosmos_msg, cw721_transfer_cosmos_msg];

    //delete offering
    OFFERINGS.remove(deps.storage, &msg.offering_id);

    let price_string = format!("{} {}", rcv_msg.amount, info.sender);

    Ok(Response::new()
        .add_messages(cosmos_msgs)
        .add_attribute("method", "buy_nft")
        .add_attribute("seller", off.seller.to_string())
        .add_attribute("buyer", rcv_msg.sender)
        .add_attribute("paid_price", price_string)
        .add_attribute("token_id", off.token_id)
        .add_attribute("contract_addr", off.contract_addr.to_string()))
}

pub fn try_receive_nft(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    rcv_msg: Cw721ReceiveMsg,
) -> Result<Response, ContractError> {
    let msg: SellNft = from_binary(&rcv_msg.msg)?;

    // check if same token Id form same original contract is already on sale
    // get OFFERING_COUNT
    let id = increment_offerings(deps.storage)?.to_string();

    // save Offering
    let off = Offering {
        contract_addr: deps.api.addr_canonicalize(&info.sender.as_str())?,
        token_id: rcv_msg.token_id,
        seller: deps.api.addr_canonicalize(&rcv_msg.sender)?,
        list_price: msg.list_price.clone(),
        listing_time: env.block.time,
    };

    OFFERINGS.save(deps.storage, &id, &off)?;

    let price_string = format!("{} {}", msg.list_price.amount, msg.list_price.address);

    Ok(Response::new()
        .add_attribute("method", "sell_nft")
        .add_attribute("original_contract", info.sender)
        .add_attribute("seller", off.seller.to_string())
        .add_attribute("list_price", price_string)
        .add_attribute("token_id", off.token_id))
}

pub fn try_withdraw(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    offering_id: String,
) -> Result<Response, ContractError> {
    // check if token_id is currently sold by the requesting address
    let off = OFFERINGS.load(deps.storage, &offering_id)?;
    if off.seller == deps.api.addr_canonicalize(&info.sender.as_str())? {
        // transfer token back to original owner
        let transfer_cw721_msg = Cw721ExecuteMsg::TransferNft {
            recipient: deps.api.addr_humanize(&off.seller)?.into_string(),
            token_id: off.token_id.clone(),
        };

        let exec_cw721_transfer = WasmMsg::Execute {
            contract_addr: deps.api.addr_humanize(&off.contract_addr)?.into_string(),
            msg: to_binary(&transfer_cw721_msg)?,
            funds: vec![],
        };

        let cw721_transfer_cosmos_msg: Vec<CosmosMsg> = vec![exec_cw721_transfer.into()];

        // remove offering
        OFFERINGS.remove(deps.storage, &offering_id);

        return Ok(Response::new()
            .add_messages(cw721_transfer_cosmos_msg)
            .add_attribute("method", "withdraw_nft")
            .add_attribute("seller", info.sender)
            .add_attribute("offering_id", offering_id));
    }
    Err(ContractError::Unauthorized {})
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetOfferings {
            sort_listing,
            index,
            size,
        } => to_binary(&query_offerings(deps, &sort_listing, index, size)?),
    }
}

fn query_offerings(
    deps: Deps,
    sort_listing: &str,
    q_index: Uint128,
    q_size: Uint128,
) -> StdResult<OfferingsResponse> {
    let res: StdResult<Vec<QueryOfferingsResult>> = OFFERINGS
        .range(deps.storage, None, None, Order::Ascending)
        .map(|kv_item| parse_offering(deps.api, kv_item))
        .collect();

    let mut offerings_clone = res?.clone();

    let index = q_index.u128() as usize;
    let size = q_size.u128() as usize;

    if offerings_clone.len() == 0 {
        return Ok(OfferingsResponse {
            total: offerings_clone.len(),
            size: size,
            index: index,
            offerings: offerings_clone,
        });
    }

    offerings_clone.sort_by(|a, b| match sort_listing {
        "price_lowest" => {
            if a.list_price.amount < b.list_price.amount {
                return Ordering::Less;
            }
            if a.list_price.amount == b.list_price.amount {
                return Ordering::Equal;
            }
            return Ordering::Greater;
        }
        "price_highest" => {
            if a.list_price.amount < b.list_price.amount {
                return Ordering::Greater;
            }
            if a.list_price.amount == b.list_price.amount {
                return Ordering::Equal;
            }
            return Ordering::Less;
        }
        "newest_listed" => {
            let a_id: u128 = a.id.parse().unwrap();
            let b_id: u128 = b.id.parse().unwrap();

            if a_id < b_id {
                return Ordering::Less;
            }
            if a_id == b_id {
                return Ordering::Equal;
            }
            return Ordering::Greater;
        }
        "oldest_listed" => {
            let a_id: u128 = a.id.parse().unwrap();
            let b_id: u128 = b.id.parse().unwrap();

            if a_id < b_id {
                return Ordering::Greater;
            }
            if a_id == b_id {
                return Ordering::Equal;
            }
            return Ordering::Less;
        }
        _ => Ordering::Equal,
    });

    let paged = Paged::new(&offerings_clone, size);
    let (_, offerings_paginated) = paged.page(index).unwrap();

    Ok(OfferingsResponse {
        total: offerings_clone.len(),
        size: size,
        index: index,
        offerings: offerings_paginated.to_vec(),
    })
}

fn parse_offering(
    api: &dyn Api,
    item: StdResult<Pair<Offering>>,
) -> StdResult<QueryOfferingsResult> {
    item.and_then(|(k, offering)| {
        let id = from_utf8(&k)?;
        Ok(QueryOfferingsResult {
            id: id.to_string(),
            token_id: offering.token_id,
            list_price: offering.list_price,
            contract_addr: api.addr_humanize(&offering.contract_addr)?.to_string(),
            seller: api.addr_humanize(&offering.seller)?.to_string(),
            listing_time: offering.listing_time,
        })
    })
}
