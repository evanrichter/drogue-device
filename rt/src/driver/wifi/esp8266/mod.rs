mod buffer;
mod num;
mod parser;
mod protocol;
mod socket_pool;

use socket_pool::SocketPool;

use crate::api::delayer::Delayer;
use crate::api::ip::tcp::{TcpError, TcpStack};
use crate::api::ip::{IpAddress, IpAddressV4, IpProtocol, SocketAddress};
use crate::api::uart::{Error as UartError, UartReader, UartWriter};
use crate::api::wifi::{Join, JoinError, WifiSupplicant};
use crate::domain::time::duration::Milliseconds;
use crate::hal::gpio::InterruptPin;
use crate::prelude::*;
use buffer::Buffer;
use core::cell::{RefCell, UnsafeCell};
use core::fmt::Write;
use cortex_m::interrupt::Nr;
use embedded_hal::digital::v2::{InputPin, OutputPin};
use heapless::{
    consts,
    spsc::{Consumer, Producer, Queue},
    String,
};
use protocol::Response as AtResponse;

pub const BUFFER_LEN: usize = 512;

#[derive(Debug)]
pub enum AdapterError {
    UnableToInitialize,
    NoAvailableSockets,
    Timeout,
    UnableToOpen,
    UnableToClose,
    WriteError,
    ReadError,
    InvalidSocket,
}

pub struct Shared {
    socket_pool: SocketPool,
}

impl Shared {
    fn new() -> Self {
        Self {
            socket_pool: SocketPool::new(),
        }
    }
}

enum State {
    Uninitialized,
    Ready,
}

pub struct Esp8266Wifi<UART, T, ENABLE, RESET>
where
    UART: UartWriter + UartReader + 'static,
    T: Delayer + 'static,
    ENABLE: OutputPin + 'static,
    RESET: OutputPin + 'static,
{
    shared: Shared,
    controller: ActorContext<Esp8266WifiController<UART, T>>,
    ingress: ActorContext<Esp8266WifiModem<UART, ENABLE, RESET>>,
    response_queue: UnsafeCell<Queue<AtResponse, consts::U2>>,
    notification_queue: UnsafeCell<Queue<AtResponse, consts::U2>>,
}

impl<UART, T, ENABLE, RESET> Esp8266Wifi<UART, T, ENABLE, RESET>
where
    UART: UartWriter + UartReader + 'static,
    T: Delayer + 'static,
    ENABLE: OutputPin + 'static,
    RESET: OutputPin + 'static,
{
    #[allow(non_camel_case_types)]
    pub fn new(enable: ENABLE, reset: RESET) -> Self {
        Self {
            shared: Shared::new(),
            controller: ActorContext::new(Esp8266WifiController::new())
                .with_name("esp8266-wifi-controller"),
            ingress: ActorContext::new(Esp8266WifiModem::new(enable, reset))
                .with_name("esp8266-wifi-ingress"),
            response_queue: UnsafeCell::new(Queue::new()),
            notification_queue: UnsafeCell::new(Queue::new()),
        }
    }
}

impl<UART, T, ENABLE, RESET> Package for Esp8266Wifi<UART, T, ENABLE, RESET>
where
    UART: UartWriter + UartReader + 'static,
    T: Delayer + 'static,
    ENABLE: OutputPin + 'static,
    RESET: OutputPin + 'static,
{
    type Primary = Esp8266WifiController<UART, T>;
    type Configuration = (Address<UART>, Address<T>);

    fn mount(
        &'static self,
        config: Self::Configuration,
        supervisor: &mut Supervisor,
    ) -> Address<Self::Primary> {
        let (r_prod, r_cons) = unsafe { (&mut *self.response_queue.get()).split() };
        let (n_prod, n_cons) = unsafe { (&mut *self.notification_queue.get()).split() };
        let addr = self.controller.mount(
            (&self.shared, r_cons, n_cons, config.0, config.1),
            supervisor,
        );
        self.ingress.mount((r_prod, n_prod, config.0), supervisor);
        addr
    }

    fn primary(&'static self) -> Address<Self::Primary> {
        self.controller.address()
    }
}

pub struct Esp8266WifiController<UART, T>
where
    UART: UartWriter + UartReader + 'static,
    T: Delayer + 'static,
{
    shared: Option<&'static Shared>,
    address: Option<Address<Self>>,
    uart: Option<Address<UART>>,
    delayer: Option<Address<T>>,
    state: State,
    response_consumer: Option<RefCell<Consumer<'static, AtResponse, consts::U2>>>,
    notification_consumer: Option<RefCell<Consumer<'static, AtResponse, consts::U2>>>,
}

impl<UART, T> Esp8266WifiController<UART, T>
where
    UART: UartWriter + UartReader + 'static,
    T: Delayer + 'static,
{
    pub fn new() -> Self {
        Self {
            address: None,
            uart: None,
            delayer: None,
            state: State::Uninitialized,
            shared: None,
            response_consumer: None,
            notification_consumer: None,
        }
    }

    async fn wait_for_response(&mut self) -> Result<AtResponse, AdapterError> {
        loop {
            if let Some(response) = self
                .response_consumer
                .as_ref()
                .unwrap()
                .borrow_mut()
                .dequeue()
            {
                return Ok(response);
            }
            self.delayer
                .as_ref()
                .unwrap()
                .delay(Milliseconds(1000))
                .await;
        }
    }

    async fn start(mut self) -> Self {
        log::info!("[{}] start", ActorInfo::name());
        self
    }
}

impl<UART, T> WifiSupplicant for Esp8266WifiController<UART, T>
where
    UART: UartWriter + UartReader + 'static,
    T: Delayer + 'static,
{
    fn join(mut self, join_info: Join) -> Response<Self, Result<IpAddress, JoinError>> {
        Response::defer(async move {
            /*
            TODO

            let result = match join_info {
                Join::Open => self.join_open().await,
                Join::Wpa { ssid, password } => {
                    self.join_wep(ssid.as_ref(), password.as_ref()).await
                }
            };*/

            (self, Err(JoinError::Unknown))
        })
    }
}

impl<UART, T> TcpStack for Esp8266WifiController<UART, T>
where
    UART: UartWriter + UartReader + 'static,
    T: Delayer + 'static,
{
    type SocketHandle = u8;

    fn open(self) -> Response<Self, Self::SocketHandle> {
        let open_future = self.shared.unwrap().socket_pool.open();
        Response::immediate_future(self, open_future)
    }

    fn connect(
        mut self,
        handle: Self::SocketHandle,
        proto: IpProtocol,
        dst: SocketAddress,
    ) -> Response<Self, Result<(), TcpError>> {
        Response::defer(async move { (self, Err(TcpError::ConnectError)) })
    }

    fn write(
        mut self,
        handle: Self::SocketHandle,
        buf: &[u8],
    ) -> Response<Self, Result<usize, TcpError>> {
        Response::immediate(self, Err(TcpError::WriteError))
    }

    fn read(
        mut self,
        handle: Self::SocketHandle,
        buf: &mut [u8],
    ) -> Response<Self, Result<usize, TcpError>> {
        Response::immediate(self, Err(TcpError::ReadError))
    }

    fn close(mut self, handle: Self::SocketHandle) -> Completion<Self> {
        Completion::immediate(self)
    }
}

impl<UART, T> Actor for Esp8266WifiController<UART, T>
where
    UART: UartWriter + UartReader + 'static,
    T: Delayer + 'static,
{
    type Configuration = (
        &'static Shared,
        Consumer<'static, AtResponse, consts::U2>,
        Consumer<'static, AtResponse, consts::U2>,
        Address<UART>,
        Address<T>,
    );

    fn on_mount(&mut self, address: Address<Self>, config: Self::Configuration)
    where
        Self: Sized,
    {
        self.shared.replace(config.0);
        self.address.replace(address);
        self.response_consumer.replace(RefCell::new(config.1));
        self.notification_consumer.replace(RefCell::new(config.2));
        self.uart.replace(config.3);
        self.delayer.replace(config.4);
    }

    fn on_start(self) -> Completion<Self>
    where
        Self: 'static,
    {
        Completion::defer(self.start())
    }
}

pub struct Esp8266WifiModem<UART, ENABLE, RESET>
where
    UART: UartReader + UartWriter + 'static,
    ENABLE: OutputPin + 'static,
    RESET: OutputPin + 'static,
{
    uart: Option<Address<UART>>,
    response_producer: Option<RefCell<Producer<'static, AtResponse, consts::U2>>>,
    notification_producer: Option<RefCell<Producer<'static, AtResponse, consts::U2>>>,
    parse_buffer: Buffer,
    enable: ENABLE,
    reset: RESET,
}

impl<UART, ENABLE, RESET> Esp8266WifiModem<UART, ENABLE, RESET>
where
    UART: UartReader + UartWriter + 'static,
    ENABLE: OutputPin + 'static,
    RESET: OutputPin + 'static,
{
    pub fn new(enable: ENABLE, reset: RESET) -> Self {
        Self {
            uart: None,
            parse_buffer: Buffer::new(),
            response_producer: None,
            notification_producer: None,
            enable,
            reset,
        }
    }

    fn digest(&mut self) -> Result<(), AdapterError> {
        let result = self.parse_buffer.parse();
        if let Ok(response) = result {
            if !matches!(response, AtResponse::None) {
                log::info!("Got response: {:?}", response);
                self.response_producer
                    .as_ref()
                    .unwrap()
                    .borrow_mut()
                    .enqueue(response)
                    .map_err(|_| AdapterError::ReadError)?;
            }
        }
        Ok(())
    }

    async fn process(&mut self) -> Result<(), AdapterError> {
        let uart = self.uart.as_ref().unwrap();

        let mut buf = [0; 1];

        let len = uart
            .read(&mut buf[..])
            .await
            .map_err(|_| AdapterError::ReadError)?;
        for b in &buf[..len] {
            self.parse_buffer.write(*b).unwrap();
        }
        Ok(())
    }

    async fn start(mut self) -> Self {
        log::info!("Starting ESP8266 Modem");
        loop {
            if let Err(e) = self.process().await {
                log::error!("Error reading data: {:?}", e);
            }

            if let Err(e) = self.digest() {
                log::error!("Error digesting data");
            }
        }
    }

    async fn initialize(mut self) -> Self {
        let mut buffer: [u8; 1024] = [0; 1024];
        let mut pos = 0;

        const READY: [u8; 7] = *b"ready\r\n";

        let mut counter = 0;

        self.enable.set_high().ok().unwrap();
        self.reset.set_high().ok().unwrap();

        log::info!("waiting for adapter to become ready");

        let mut rx_buf = [0; 1];
        loop {
            let result = self.uart.unwrap().read(&mut rx_buf[..]).await;
            match result {
                Ok(c) => {
                    buffer[pos] = rx_buf[0];
                    pos += 1;
                    if pos >= READY.len() && buffer[pos - READY.len()..pos] == READY {
                        log::info!("adapter is ready");
                        self.disable_echo()
                            .await
                            .map_err(|e| log::error!("Error disabling echo mode"));
                        log::info!("Echo disabled");
                        self.enable_mux()
                            .await
                            .map_err(|e| log::error!("Error enabling mux"));
                        log::info!("Mux enabled");
                        self.set_recv_mode()
                            .await
                            .map_err(|e| log::error!("Error setting receive mode"));
                        log::info!("adapter configured");
                        break;
                    }
                }
                Err(e) => {
                    log::error!("Error initializing ESP8266 modem");
                    break;
                }
            }
        }
        self
    }

    async fn write_command(&self, cmd: &[u8]) -> Result<(), UartError> {
        self.uart.as_ref().unwrap().write(cmd).await
    }

    async fn disable_echo(&self) -> Result<(), AdapterError> {
        self.write_command(b"ATE0\r\n")
            .await
            .map_err(|_| AdapterError::UnableToInitialize)?;
        Ok(self
            .wait_for_ok()
            .await
            .map_err(|_| AdapterError::UnableToInitialize)?)
    }

    async fn enable_mux(&self) -> Result<(), AdapterError> {
        self.write_command(b"AT+CIPMUX=1\r\n")
            .await
            .map_err(|_| AdapterError::UnableToInitialize)?;
        Ok(self
            .wait_for_ok()
            .await
            .map_err(|_| AdapterError::UnableToInitialize)?)
    }

    async fn set_recv_mode(&self) -> Result<(), AdapterError> {
        self.write_command(b"AT+CIPRECVMODE=1\r\n")
            .await
            .map_err(|_| AdapterError::UnableToInitialize)?;
        Ok(self
            .wait_for_ok()
            .await
            .map_err(|_| AdapterError::UnableToInitialize)?)
    }

    async fn wait_for_ok(&self) -> Result<(), AdapterError> {
        let mut buf: [u8; 64] = [0; 64];
        let mut pos = 0;

        loop {
            self.uart
                .as_ref()
                .unwrap()
                .read(&mut buf[pos..pos + 1])
                .await
                .map_err(|_| AdapterError::ReadError)?;
            pos += 1;
            if buf[0..pos].ends_with(b"OK\r\n") {
                return Ok(());
            } else if buf[0..pos].ends_with(b"ERROR\r\n") {
                return Err(AdapterError::UnableToInitialize);
            }
        }
    }
}

impl<UART, ENABLE, RESET> Actor for Esp8266WifiModem<UART, ENABLE, RESET>
where
    UART: UartReader + UartWriter + 'static,
    ENABLE: OutputPin + 'static,
    RESET: OutputPin + 'static,
{
    type Configuration = (
        Producer<'static, AtResponse, consts::U2>,
        Producer<'static, AtResponse, consts::U2>,
        Address<UART>,
    );

    fn on_mount(&mut self, address: Address<Self>, config: Self::Configuration)
    where
        Self: Sized,
    {
        self.response_producer.replace(RefCell::new(config.0));
        self.notification_producer.replace(RefCell::new(config.1));
        self.uart.replace(config.2);
    }

    fn on_initialize(mut self) -> Completion<Self>
    where
        Self: 'static,
    {
        Completion::defer(self.initialize())
    }

    fn on_start(self) -> Completion<Self>
    where
        Self: 'static,
    {
        Completion::defer(self.start())
    }
}
