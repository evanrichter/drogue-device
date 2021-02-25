#![no_main]
#![no_std]

mod device;

use panic_rtt_target as _;

use cortex_m_rt::{entry, exception};
use drogue_device::{
    domain::time::duration::Milliseconds,
    driver::{
        led::{Blinker, SimpleLED},
        timer::Timer,
    },
    hal::Active,
    platform::cortex_m::nrf::timer::Timer as NrfTimer,
    prelude::*,
};
use hal::gpio::Level;
use hal::pac::interrupt;
use hal::pac::NVIC;
use log::LevelFilter;
use rtt_logger::RTTLogger;
use rtt_target::rtt_init_print;

use nrf51_hal as hal;

use crate::device::*;

static LOGGER: RTTLogger = RTTLogger::new(LevelFilter::Info);

#[entry]
fn main() -> ! {
    rtt_init_print!();
    unsafe {
        log::set_logger_racy(&LOGGER).unwrap();
    }
    log::set_max_level(log::LevelFilter::Info);

    let device = hal::pac::Peripherals::take().unwrap();

    let port0 = hal::gpio::p0::Parts::new(device.GPIO);

    let clocks = hal::clocks::Clocks::new(device.CLOCK).enable_ext_hfosc();
    let _clocks = clocks.start_lfclk();

    let led = SimpleLED::new(
        port0.p0_21.into_push_pull_output(Level::Low).degrade(),
        Active::High,
    );

    let blinker = Blinker::new(Milliseconds(500u32));

    let timer = Timer::new(NrfTimer::new(device.TIMER0), hal::pac::Interrupt::TIMER0);

    let device = MyDevice {
        led: ActorContext::new(led).with_name("led"),
        blinker: ActorContext::new(blinker).with_name("blinker"),
        timer,
    };

    device!( MyDevice = device; 1024);
}
