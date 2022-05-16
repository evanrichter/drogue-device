pub mod rng;

use crate::actors::ble::mesh::NodeMutex;
pub use crate::drivers::ble::mesh::bearer::nrf52::rng::SoftdeviceRng;
use crate::drivers::ble::mesh::driver::node::{NetworkId, State};
use crate::drivers::ble::mesh::interface::{AdvertisingBearer, BearerError};
use crate::drivers::ble::mesh::interface::{GattBearer, PB_ADV_MTU};
use crate::drivers::ble::mesh::{MESH_MESSAGE, PB_ADV};
use atomic_polyfill::AtomicBool;
use core::cell::Cell;
use core::cell::RefCell;
use core::future::Future;
use core::mem;
use core::ptr::slice_from_raw_parts;
use core::sync::atomic::Ordering;
use embassy::channel::{Channel, Signal};
use heapless::Vec;
use nrf_softdevice::ble::central::{ScanConfig, ScanError};
use nrf_softdevice::ble::peripheral::AdvertiseError;
use nrf_softdevice::ble::{central, gatt_server, peripheral, Connection};
use nrf_softdevice::{raw, Flash, Softdevice};

pub struct Nrf52BleMeshFacilities {
    pub(crate) sd: &'static Softdevice,
}

impl Nrf52BleMeshFacilities {
    pub fn new(device_name: &'static str) -> Self {
        Self {
            sd: Self::new_sd(device_name),
        }
    }

    pub fn new_sd(device_name: &'static str) -> &'static Softdevice {
        let config = nrf_softdevice::Config {
            clock: Some(raw::nrf_clock_lf_cfg_t {
                source: raw::NRF_CLOCK_LF_SRC_RC as u8,
                rc_ctiv: 4,
                rc_temp_ctiv: 2,
                accuracy: 7,
            }),
            conn_gap: Some(raw::ble_gap_conn_cfg_t {
                conn_count: 1,
                event_length: 24,
            }),
            conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 517 }),
            gatts_attr_tab_size: Some(raw::ble_gatts_cfg_attr_tab_size_t {
                attr_tab_size: 32768,
            }),
            gap_role_count: Some(raw::ble_gap_cfg_role_count_t {
                adv_set_count: 1,
                periph_role_count: 2,
                central_role_count: 2,
                central_sec_count: 2,
                _bitfield_1: Default::default(),
            }),
            gap_device_name: Some(raw::ble_gap_cfg_device_name_t {
                p_value: device_name.as_ptr() as *const u8 as _,
                current_len: device_name.len() as u16,
                max_len: device_name.len() as u16,
                write_perm: unsafe { mem::zeroed() },
                _bitfield_1: raw::ble_gap_cfg_device_name_t::new_bitfield_1(
                    raw::BLE_GATTS_VLOC_STACK as u8,
                ),
            }),

            ..Default::default()
        };
        let sd = Softdevice::enable(&config);
        sd
    }

    pub fn advertising_bearer(&self) -> SoftdeviceAdvertisingBearer {
        SoftdeviceAdvertisingBearer::new(self.sd)
    }

    pub fn gatt_bearer(&self) -> SoftdeviceGattBearer {
        SoftdeviceGattBearer::new(self.sd)
    }

    pub fn rng(&self) -> SoftdeviceRng {
        SoftdeviceRng::new(self.sd)
    }

    pub fn flash(&self) -> Flash {
        Flash::take(self.sd)
    }
}

pub struct SoftdeviceGattBearer {
    sd: &'static Softdevice,
    connection: Signal<Connection>,
    current_connection: RefCell<Option<Connection>>,
    provisioning_server: ProvisioningServer,
    proxy_server: ProxyServer,
    connected: AtomicBool,
    outbound: Channel<NodeMutex, Vec<u8, 66>, 5>,
    inbound: Channel<NodeMutex, Vec<u8, 66>, 5>,
    state: Cell<State>,
    network_id: Cell<Option<NetworkId>>,
}

impl SoftdeviceGattBearer {
    pub fn new(sd: &'static Softdevice) -> Self {
        Self {
            sd,
            provisioning_server: gatt_server::register(sd).unwrap(),
            proxy_server: gatt_server::register(sd).unwrap(),
            connection: Signal::new(),
            connected: AtomicBool::new(false),
            current_connection: RefCell::new(None),
            outbound: Channel::new(),
            inbound: Channel::new(),
            state: Cell::new(State::Unprovisioned),
            network_id: Cell::new(None),
        }
    }

    async fn run(&self) -> Result<(), BearerError> {
        loop {
            if let State::Provisioned = self.state.get() {
                let connection = self.connection.wait().await;
                self.current_connection.borrow_mut().replace(connection);
                gatt_server::run(
                    self.current_connection.borrow().as_ref().unwrap(),
                    &self.proxy_server,
                    |e| match e {
                        ProxyServerEvent::Proxy(event) => match event {
                            ProxyServiceEvent::DataInWrite(data) => {
                                self.inbound.try_send(data).ok();
                            }
                            ProxyServiceEvent::DataOutCccdWrite { .. } => {}
                            _ => {
                                // ignorable
                            }
                        },
                    },
                )
                .await
                .ok();
                self.current_connection.borrow_mut().take();
                self.connected.store(false, Ordering::Relaxed);
            } else {
                let connection = self.connection.wait().await;
                self.current_connection.borrow_mut().replace(connection);
                gatt_server::run(
                    self.current_connection.borrow().as_ref().unwrap(),
                    &self.provisioning_server,
                    |e| match e {
                        ProvisioningServerEvent::Provisioning(event) => match event {
                            ProvisioningServiceEvent::DataInWrite(data) => {
                                self.inbound.try_send(data).ok();
                            }
                            ProvisioningServiceEvent::DataOutCccdWrite { .. } => {}
                            _ => {
                                // ignorable
                            }
                        },
                    },
                )
                .await
                .ok();
                self.current_connection.borrow_mut().take();
                self.connected.store(false, Ordering::Relaxed);
            }
        }
    }
}

pub const ATT_MTU: usize = 69;

impl GattBearer<66> for SoftdeviceGattBearer {
    fn set_network_id(&self, network_id: NetworkId) {
        self.network_id.replace(Some(network_id));
    }

    fn set_state(&self, state: State) {
        self.state.replace(state);
    }

    type RunFuture<'m> = impl Future<Output=Result<(), BearerError>> + 'm
    where
    Self: 'm;

    fn run<'m>(&'m self) -> Self::RunFuture<'m> {
        SoftdeviceGattBearer::run(self)
    }

    type ReceiveFuture<'m> = impl Future<Output=Result<Vec<u8, 66>, BearerError>> + 'm
    where
    Self: 'm;

    fn receive<'m>(&'m self) -> Self::ReceiveFuture<'m> {
        async move {
            loop {
                return Ok(self.inbound.recv().await);
            }
        }
    }

    type TransmitFuture<'m> = impl Future<Output = Result<(), BearerError>> + 'm;

    fn transmit<'m>(&'m self, pdu: &'m Vec<u8, 66>) -> Self::TransmitFuture<'m> {
        //async move { Ok(()) }
        async move {
            if let Some(connection) = self.current_connection.borrow().as_ref() {
                match self.state.get() {
                    State::Unprovisioned | State::Provisioning => {
                        self.provisioning_server
                            .provisioning
                            .data_out_notify(&connection, pdu.clone())
                            .map_err(|_| BearerError::TransmissionFailure)?;
                    }
                    State::Provisioned => {
                        self.proxy_server
                            .proxy
                            .data_out_notify(&connection, pdu.clone())
                            .map_err(|_| BearerError::TransmissionFailure)?;
                    }
                }
            }

            Ok(())
        }
    }

    type AdvertiseFuture<'m> = impl Future<Output = Result<(), BearerError>> + 'm;

    fn advertise<'m>(&'m self, adv_data: &'m Vec<u8, 64>) -> Self::AdvertiseFuture<'m> {
        async move {
            if self.connected.load(Ordering::Relaxed) {
                return Ok(());
            }
            let scan_data: Vec<u8, 16> = Vec::new();
            //scan_data.push(4)?;
            //scan_data.push(0x09)?;
            //scan_data.extend_from_slice(b"Bob")?;

            let adv = peripheral::ConnectableAdvertisement::ScannableUndirected {
                adv_data,
                scan_data: &scan_data,
            };
            let result = peripheral::advertise_connectable(
                self.sd,
                adv,
                &peripheral::Config {
                    timeout: Some(15),
                    interval: 50,
                    ..Default::default()
                },
            )
            .await;
            match result {
                Ok(connection) => {
                    self.connected.store(true, Ordering::Relaxed);
                    self.connection.signal(connection);
                    return Ok(());
                }
                Err(err) => match err {
                    AdvertiseError::Timeout => {}
                    AdvertiseError::NoFreeConn => {}
                    AdvertiseError::Raw(_) => {}
                },
            }
            Ok(())
        }
    }
}

pub struct SoftdeviceAdvertisingBearer {
    sd: &'static Softdevice,
}

impl SoftdeviceAdvertisingBearer {
    pub fn new(sd: &'static Softdevice) -> Self {
        Self { sd }
    }
}

impl AdvertisingBearer for SoftdeviceAdvertisingBearer {
    type TransmitFuture<'m> = impl Future<Output = Result<(), BearerError>> + 'm;

    fn transmit<'m>(&'m self, message: &'m Vec<u8, PB_ADV_MTU>) -> Self::TransmitFuture<'m> {
        let adv =
            peripheral::NonconnectableAdvertisement::NonscannableUndirected { adv_data: message };

        async move {
            if let Err(err) = peripheral::advertise(
                self.sd,
                adv,
                &peripheral::Config {
                    max_events: Some(3),
                    interval: 50,
                    ..Default::default()
                },
            )
            .await
            {
                match err {
                    AdvertiseError::Timeout => Ok(()),
                    AdvertiseError::NoFreeConn => Err(BearerError::InsufficientResources),
                    AdvertiseError::Raw(_) => Err(BearerError::TransmissionFailure),
                }
            } else {
                Ok(())
            }
        }
    }

    type ReceiveFuture<'m> = impl Future<Output=Result<Vec<u8, PB_ADV_MTU>, BearerError>> + 'm
    where
    Self: 'm;

    fn receive<'m>(&'m self) -> Self::ReceiveFuture<'m> {
        async move {
            //let config = ScanConfig::default();
            let config = ScanConfig {
                active: false,
                interval: 50,
                window: 100,
                ..Default::default()
            };
            loop {
                let result = central::scan::<_, Vec<u8, PB_ADV_MTU>>(self.sd, &config, |event| {
                    let data = event.data;
                    let data = unsafe { &*slice_from_raw_parts(data.p_data, data.len as usize) };
                    if data.len() >= 2 && (data[1] == PB_ADV || data[1] == MESH_MESSAGE) {
                        Some(Vec::from_slice(data).unwrap())
                    } else {
                        None
                    }
                })
                .await;

                match result {
                    Ok(data) => {
                        return Ok(data);
                    }
                    Err(err) => {
                        match err {
                            ScanError::Timeout => { /* ignore, loop */ }
                            ScanError::Raw(_) => {
                                return Err(BearerError::Unspecified);
                            }
                        }
                    }
                }
            }
        }
    }

    fn set_state(&self, _state: State) {
        // ignored.
    }

    fn set_network_id(&self, _network_id: NetworkId) {
        // ignored
    }
}

#[nrf_softdevice::gatt_server]
pub struct ProvisioningServer {
    provisioning: ProvisioningService,
}

#[nrf_softdevice::gatt_service(uuid = "1827")]
pub struct ProvisioningService {
    #[characteristic(uuid = "2adb", write_without_response)]
    pub data_in: Vec<u8, 66>,
    #[characteristic(uuid = "2adc", read, write, notify)]
    pub data_out: Vec<u8, 66>,
}

#[nrf_softdevice::gatt_server]
pub struct ProxyServer {
    proxy: ProxyService,
}

#[nrf_softdevice::gatt_service(uuid = "1828")]
pub struct ProxyService {
    #[characteristic(uuid = "2add", write_without_response)]
    pub data_in: Vec<u8, 66>,
    #[characteristic(uuid = "2ade", read, write, notify)]
    pub data_out: Vec<u8, 66>,
}
