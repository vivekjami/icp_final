#![allow(clippy::collapsible_else_if)]
#[macro_use]
extern crate ic_cdk_macros;
#[macro_use]
extern crate serde;

mod http;
mod types;
use crate::types::WeatherCondition;
use std::{borrow::Cow, collections::HashMap};
use std::cell::RefCell;
use std::collections::HashSet;
use std::convert::TryFrom;
use std::iter::FromIterator;
use std::mem;

use std::str::FromStr;
use candid::{Encode, Principal};
use ic_cdk::{
    api::{self, call},
    export::candid,
    storage,
};
use include_base64::include_base64;
use types::{
    ConstrainedError, Error, ExtendedMetadataResult, InitArgs, InterfaceId, LogoResult, MetadataDesc, MetadataPart, MetadataPurpose, MetadataVal, MintResult, Nft, Result, StableState, State
};

use ic_cdk::api::management_canister::http_request::{
    http_request, CanisterHttpRequestArgument, HttpMethod,
};

use candid::{CandidType, Decode, Deserialize};
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::{BoundedStorable, DefaultMemoryImpl, StableBTreeMap, Storable};
use serde::Serialize;


//weather condition struct for deserializing response
#[derive(CandidType, Deserialize, Clone)]
#[derive(Default, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub id:f64,
    pub main: String,
    pub description: String,
}

//wind info struct for deserializing response
#[derive(CandidType, Deserialize, Clone)]
#[derive(Default, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Wind {
    pub speed: f64,
    pub deg: f64,
}

// main data struct for deserializing response
#[derive(CandidType, Deserialize, Clone)]
#[derive(Default, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Main {
    pub temp: f64,
    #[serde(rename = "feels_like")]
    pub feels_like: f64,
    #[serde(rename = "temp_min")]
    pub temp_min: f64,
    #[serde(rename = "temp_max")]
    pub temp_max: f64,
    pub pressure: f64,
    pub humidity: f64,
}

//base struct
#[derive(CandidType, Deserialize, Clone)]
#[derive(Default, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Weather {
    pub main: Main,
    pub weather: Vec<Condition>,
    pub wind:Wind

}

// Implementing Storable for Weather
impl Storable for Weather {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(Encode!(self).unwrap())
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).unwrap()
    }
}

//memory definition
type Memory = VirtualMemory<DefaultMemoryImpl>;
const MAX_VALUE_SIZE: u32 = 100;

// Implementing BoundedStorable for Weather
impl BoundedStorable for Weather {
    const MAX_SIZE: u32 = MAX_VALUE_SIZE;
    const IS_FIXED_SIZE: bool = false;
}

// Creating memory manager with a new MemoryId
thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
    RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static WEATHER_MAP: RefCell<StableBTreeMap<u64, Weather,Memory>> = RefCell::new(
        StableBTreeMap::init(
            MEMORY_MANAGER.with(|m| m.borrow().get(MemoryId::new(1))), // Use a different MemoryId if needed
        )
    );
}

//getting weather data by city name
#[ic_cdk::update]
async fn get_weather_by_city_name(city_name: String) -> Result<Weather,String> {
    let api_endpoint = "https://api.openweathermap.org/data/2.5/weather?q=";
    let new_city_name = city_name.replace(" ", "%20");

    //key value to access
    let api_key = "c75ba923f1f8e9130caee26bbfbbebca";

    // url to be sent
    let url = format!("{}{}&units=metric&APPID={}", api_endpoint, new_city_name, api_key);

    let request_headers = vec![];

    // setting the request params,headers and body
    let request = CanisterHttpRequestArgument {
        url,
        method: HttpMethod::GET,
        body: None,
        max_response_bytes: None,
        transform: None,
        headers: request_headers,
    };

    //if Ok => deserializing response
    //else => returning a string message
    match http_request(request).await {
        Ok((response,)) => {
            if response.status == 200 {
                let weather_data: Weather =
                    serde_json::from_slice(&response.body).expect("Failed to parse JSON response.");
                
                return Ok(weather_data);
            } else {
                Err(format!("HTTP request failed with status code: {}", response.status))
            }
        }
        Err((code, message)) => {
            Err(format!(
                "The http_request resulted in an error. Code: {:?}, Message: {}",
                code, message
            ))
        }
    }
}


const MGMT: Principal = Principal::from_slice(&[]);

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

//Prepares and serializes the canister's state before an upgrade, ensuring no data is lost during the upgrade process.
#[pre_upgrade]
fn pre_upgrade() {
    let state = STATE.with(|state| mem::take(&mut *state.borrow_mut()));
    let hashes = http::HASHES.with(|hashes| mem::take(&mut *hashes.borrow_mut()));
    let hashes = hashes.iter().map(|(k, v)| (k.clone(), *v)).collect();
    let stable_state = StableState { state, hashes };
    storage::stable_save((stable_state,)).unwrap();
}
//Restores the canister's state after an upgrade, ensuring continuity of the canister's operation with the same data as before.
#[post_upgrade]
fn post_upgrade() {
    let (StableState { state, hashes },) = storage::stable_restore().unwrap();
    STATE.with(|state0| *state0.borrow_mut() = state);
    let hashes = hashes.into_iter().collect();
    http::HASHES.with(|hashes0| *hashes0.borrow_mut() = hashes);
}

#[init]
fn init(args: InitArgs) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.custodians = args
            .custodians
            .unwrap_or_else(|| HashSet::from_iter([api::caller()]));
        state.name = args.name;
        state.symbol = args.symbol;
        state.logo = args.logo;
    });
}
// --------------
// mint interface
// --------------

//Minting Nft and checking if city name and its weather condition matches with the given parameters
#[update(name = "mintDip721")]
async fn  mint(
    principal_data: String,
    city: String,
    weather_condition: String
) -> Result<MintResult, ConstrainedError> {
    
    //checking if given city name exists
    let city_result = get_weather_by_city_name(city.clone()).await;
    match city_result {
        Ok(data) => {

            //checking if given weather conditions typed correctly
            let weather_condition_enum = get_weather_condition(&weather_condition);
            match &weather_condition_enum {
            Ok(_data) => &weather_condition_enum,
            Err(_error) => return Err(ConstrainedError::NoSuchWeatherCondition)

        };
            //checking if given city's current weather condition matches with the given weather condition
            let weather_condition_check = check_weather_condition(data.weather.first().unwrap().main.clone().as_str(), &weather_condition_enum.as_ref().unwrap());
            if(! weather_condition_check){
                return Err(ConstrainedError::WeatherConditionNotMatched);
            }

            let to = Principal::from_str(&principal_data).unwrap();
            let mut hash_map:HashMap<String, MetadataVal> = HashMap::new();
            hash_map.insert("contentType".to_string(), MetadataVal::TextContent("text/plain".to_string()));
            hash_map.insert("locationType".to_string(), MetadataVal::Nat8Content(4));
            // let blob_data:Vec::<u8> = Blob::from_str(&weather_condition).unwrap().to_vec();
            
            let metadata_part = MetadataPart{
                key_val_data: hash_map,
                data:Vec::<u8>::new(),
                purpose:MetadataPurpose::Rendered,
                city:city,
                weather_condition:weather_condition_enum.unwrap()
    
            };
            let mut metadata: Vec<MetadataPart> = Vec::<MetadataPart>::new();
            metadata.insert(0, metadata_part);

            let (txid, tkid) = STATE.with(|state| {
                let mut state = state.borrow_mut();
                let new_id = state.nfts.len() as u64;
                let nft = Nft {
                    owner: to,
                    approved: None,
                    id: new_id,
                    metadata,
                    content: Vec::<u8>::new(),
                };
                state.nfts.push(nft);
                Ok((state.next_txid(), new_id))
            })?;
            http::add_hash(tkid);
            Ok(MintResult {
                id: txid,
                token_id: tkid,
                is_successful:true,
            })
        },
        Err(err) => Err(ConstrainedError::CityNotFound)

    }

    
}

//checking if given weather conditions typed correctly
fn get_weather_condition(data:&str) -> Result<WeatherCondition,Error>{
    match data {
        "Thunderstorm" => Ok(WeatherCondition::Thunderstorm),
        "Drizzle" => Ok(WeatherCondition::Drizzle),
        "Rain" => Ok(WeatherCondition::Rain),
        "Snow" => Ok(WeatherCondition::Snow),
        "Atmosphere" => Ok(WeatherCondition::Atmosphere),
        "Clear" => Ok(WeatherCondition::Clear),
        "Clouds" => Ok(WeatherCondition::Clouds),
        _ => Err(Error::Other),
    }
}
//checking if given city's current weather condition matches with the given weather condition
fn check_weather_condition(weather_result:&str, enum_result:&WeatherCondition ) ->bool{

    match enum_result {
        WeatherCondition::Atmosphere =>{
            match weather_result {
                "Atmosphere" => true,
                _ => false
            }
        },
        WeatherCondition::Thunderstorm =>{
            match weather_result {
                "Thunderstorm" => true,
                _ => false
            }
        },
        WeatherCondition::Drizzle =>{
            match weather_result {
                "Drizzle" => true,
                _ => false
            }
        },
        WeatherCondition::Rain =>{
            match weather_result {
                "Rain" => true,
                _ => false
            }
        },
        WeatherCondition::Snow =>{
            match weather_result {
                "Snow" => true,
                _ => false
            }
        },
        WeatherCondition::Clear =>{
            match weather_result {
                "Clear" => true,
                _ => false
            }
        },
        WeatherCondition::Clouds =>{
            match weather_result {
                "Clouds" => true,
                _ => false
            }
        },
    }
}
// --------------
// base interface
// --------------

#[query(name = "balanceOfDip721")]
fn balance_of(user: Principal) -> u64 {
    STATE.with(|state| {
        state
            .borrow()
            .nfts
            .iter()
            .filter(|n| n.owner == user)
            .count() as u64
    })
}

#[query(name = "ownerOfDip721")]
fn owner_of(token_id: u64) -> Result<Principal> {
    STATE.with(|state| {
        let owner = state
            .borrow()
            .nfts
            .get(usize::try_from(token_id)?)
            .ok_or(Error::InvalidTokenId)?
            .owner;
        Ok(owner)
    })
}

#[update(name = "transferFromDip721")]
fn transfer_from(from: Principal, to: Principal, token_id: u64) -> Result {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = &mut *state;
        let nft = state
            .nfts
            .get_mut(usize::try_from(token_id)?)
            .ok_or(Error::InvalidTokenId)?;
        let caller = api::caller();
        if nft.owner != caller
            && nft.approved != Some(caller)
            && !state
                .operators
                .get(&from)
                .map(|s| s.contains(&caller))
                .unwrap_or(false)
            && !state.custodians.contains(&caller)
        {
            Err(Error::Unauthorized)
        } else if nft.owner != from {
            Err(Error::Other)
        } else {
            nft.approved = None;
            nft.owner = to;
            Ok(state.next_txid())
        }
    })
}

#[update(name = "safeTransferFromDip721")]
fn safe_transfer_from(from: Principal, to: Principal, token_id: u64) -> Result {
    if to == MGMT {
        Err(Error::ZeroAddress)
    } else {
        transfer_from(from, to, token_id)
    }
}

#[query(name = "supportedInterfacesDip721")]
fn supported_interfaces() -> &'static [InterfaceId] {
    &[
        InterfaceId::TransferNotification,
        InterfaceId::Approval, // Psychedelic/DIP721#5
        InterfaceId::Burn,
        InterfaceId::Mint,
    ]
}

#[export_name = "canister_query logoDip721"]
fn logo() /* -> &'static LogoResult */
{
    ic_cdk::setup();
    STATE.with(|state| call::reply((state.borrow().logo.as_ref().unwrap_or(&DEFAULT_LOGO),)))
}

#[query(name = "nameDip721")]
fn name() -> String {
    STATE.with(|state| state.borrow().name.clone())
}

#[query(name = "symbolDip721")]
fn symbol() -> String {
    STATE.with(|state| state.borrow().symbol.clone())
}

const DEFAULT_LOGO: LogoResult = LogoResult {
    data: Cow::Borrowed(include_base64!("logo.png")),
    logo_type: Cow::Borrowed("image/png"),
};

#[query(name = "totalSupplyDip721")]
fn total_supply() -> u64 {
    STATE.with(|state| state.borrow().nfts.len() as u64)
}

#[export_name = "canister_query getMetadataDip721"]
fn get_metadata(/* token_id: u64 */) /* -> Result<&'static MetadataDesc> */
{
    ic_cdk::setup();
    let token_id = call::arg_data::<(u64,)>().0;
    let res: Result<()> = STATE.with(|state| {
        let state = state.borrow();
        let metadata = &state
            .nfts
            .get(usize::try_from(token_id)?)
            .ok_or(Error::InvalidTokenId)?
            .metadata;
        call::reply((Ok::<_, Error>(metadata),));
        Ok(())
    });
    if let Err(e) = res {
        call::reply((Err::<MetadataDesc, _>(e),));
    }
}


#[export_name = "canister_query getCity"]
fn get_city() 
{
    ic_cdk::setup();
    let token_id = call::arg_data::<(u64,)>().0;
    let res: Result<()> = STATE.with(|state| {
        let state = state.borrow();
        let metadata = &state
            .nfts
            .get(usize::try_from(token_id)?)
            .ok_or(Error::Other)?
            .metadata;
        let city = (metadata.first().ok_or(Error::Other))?.city.to_string();

        call::reply((city,));
        Ok(())
    });
    if let Err(e) = res {
        call::reply(("Not found".to_string(),));
    }
}

#[export_name = "canister_query getWeatherCondition"]
fn get_Weather_condition() 
{
    ic_cdk::setup();
    let token_id = call::arg_data::<(u64,)>().0;
    let res: Result<()> = STATE.with(|state| {
        let state = state.borrow();
        let metadata = &state
            .nfts
            .get(usize::try_from(token_id)?)
            .ok_or(Error::Other)?
            .metadata;
        let weather_condition = (metadata.first().ok_or(Error::Other))?.weather_condition.clone();
        let mut data = "".to_string();
        match weather_condition {
            WeatherCondition::Atmosphere => call::reply(("Atmosphere".to_string(),)),
            WeatherCondition::Clear => call::reply(("Clear".to_string(),)),
            WeatherCondition::Clouds => call::reply(("Clouds".to_string(),)),
            WeatherCondition::Drizzle => call::reply(("Drizzle".to_string(),)),
            WeatherCondition::Rain => call::reply(("Rain".to_string(),)),
            WeatherCondition::Snow => call::reply(("Snow".to_string(),)),
            WeatherCondition::Thunderstorm => call::reply(("Thunderstorm".to_string(),)),
        }
        Ok(())
    });
    if let Err(e) = res {
        call::reply(("An Error occurred".to_string(),));
    }
}


#[export_name = "canister_update getMetadataForUserDip721"]
fn get_metadata_for_user(/* user: Principal */) /* -> Vec<ExtendedMetadataResult> */
{
    ic_cdk::setup();
    let user = call::arg_data::<(Principal,)>().0;
    STATE.with(|state| {
        let state = state.borrow();
        let metadata: Vec<_> = state
            .nfts
            .iter()
            .filter(|n| n.owner == user)
            .map(|n| ExtendedMetadataResult {
                metadata_desc: &n.metadata,
                token_id: n.id,
            })
            .collect();
        call::reply((metadata,));
    });
}

// ----------------------
// notification interface
// ----------------------

#[update(name = "transferFromNotifyDip721")]
fn transfer_from_notify(from: Principal, to: Principal, token_id: u64, data: Vec<u8>) -> Result {
    let res = transfer_from(from, to, token_id)?;
    if let Ok(arg) = Encode!(&api::caller(), &from, &token_id, &data) {
        // Using call_raw ensures we don't need to await the future for the call to be executed.
        // Calling an arbitrary function like this means that a malicious recipient could call
        // transferFromNotifyDip721 in their onDIP721Received function, resulting in an infinite loop.
        // This will trap eventually, but the transfer will have already been completed and the state-change persisted.
        // That means the original transfer must reply before that happens, or the caller will be
        // convinced that the transfer failed when it actually succeeded. So we don't await the call,
        // so that we'll reply immediately regardless of how long the notification call takes.
        let _ = api::call::call_raw(to, "onDIP721Received", &arg, 0);
    }
    Ok(res)
}

#[update(name = "safeTransferFromNotifyDip721")]
fn safe_transfer_from_notify(
    from: Principal,
    to: Principal,
    token_id: u64,
    data: Vec<u8>,
) -> Result {
    if to == MGMT {
        Err(Error::ZeroAddress)
    } else {
        transfer_from_notify(from, to, token_id, data)
    }
}

// ------------------
// approval interface
// ------------------

#[update(name = "approveDip721")]
fn approve(user: Principal, token_id: u64) -> Result {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = &mut *state;
        let caller = api::caller();
        let nft = state
            .nfts
            .get_mut(usize::try_from(token_id)?)
            .ok_or(Error::InvalidTokenId)?;
        if nft.owner != caller
            && nft.approved != Some(caller)
            && !state
                .operators
                .get(&user)
                .map(|s| s.contains(&caller))
                .unwrap_or(false)
            && !state.custodians.contains(&caller)
        {
            Err(Error::Unauthorized)
        } else {
            nft.approved = Some(user);
            Ok(state.next_txid())
        }
    })
}

#[update(name = "setApprovalForAllDip721")]
fn set_approval_for_all(operator: Principal, is_approved: bool) -> Result {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let caller = api::caller();
        if operator != caller {
            let operators = state.operators.entry(caller).or_default();
            if operator == MGMT {
                if !is_approved {
                    operators.clear();
                } else {
                    // cannot enable everyone as an operator
                }
            } else {
                if is_approved {
                    operators.insert(operator);
                } else {
                    operators.remove(&operator);
                }
            }
        }
        Ok(state.next_txid())
    })
}

// #[query(name = "getApprovedDip721")] // Psychedelic/DIP721#5
fn _get_approved(token_id: u64) -> Result<Principal> {
    STATE.with(|state| {
        let approved = state
            .borrow()
            .nfts
            .get(usize::try_from(token_id)?)
            .ok_or(Error::InvalidTokenId)?
            .approved
            .unwrap_or_else(api::caller);
        Ok(approved)
    })
}

#[query(name = "isApprovedForAllDip721")]
fn is_approved_for_all(operator: Principal) -> bool {
    STATE.with(|state| {
        state
            .borrow()
            .operators
            .get(&api::caller())
            .map(|s| s.contains(&operator))
            .unwrap_or(false)
    })
}



// --------------
// burn interface
// --------------

#[update(name = "burnDip721")]
fn burn(token_id: u64) -> Result {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let nft = state
            .nfts
            .get_mut(usize::try_from(token_id)?)
            .ok_or(Error::InvalidTokenId)?;
        if nft.owner != api::caller() {
            Err(Error::Unauthorized)
        } else {
            nft.owner = MGMT;
            Ok(state.next_txid())
        }
    })
}

#[update]
fn set_name(name: String) -> Result<()> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.custodians.contains(&api::caller()) {
            state.name = name;
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    })
}

#[update]
fn set_symbol(sym: String) -> Result<()> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.custodians.contains(&api::caller()) {
            state.symbol = sym;
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    })
}

#[update]
fn set_logo(logo: Option<LogoResult>) -> Result<()> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.custodians.contains(&api::caller()) {
            state.logo = logo;
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    })
}

#[update]
fn set_custodian(user: Principal, custodian: bool) -> Result<()> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.custodians.contains(&api::caller()) {
            if custodian {
                state.custodians.insert(user);
            } else {
                state.custodians.remove(&user);
            }
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    })
}

#[query]
fn is_custodian(principal: Principal) -> bool {
    STATE.with(|state| state.borrow().custodians.contains(&principal))
}
