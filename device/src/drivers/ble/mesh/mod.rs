pub const PB_ADV: u8 = 0x29;
pub const MESH_MESSAGE: u8 = 0x2A;
pub const MESH_BEACON: u8 = 0x2B;

pub mod address;
pub mod app;
pub mod beacon;
pub mod bearer;
pub mod composition;
pub mod config;
pub mod crypto;
pub mod device;
pub mod driver;
pub mod generic_provisioning;
pub mod interface;
pub mod model;
pub mod pdu;
pub mod provisioning;
pub mod status;
pub mod storage;
pub mod vault;

#[derive(Debug)]
pub struct InsufficientBuffer;
