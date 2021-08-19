use core::future::Future;
use core::pin::Pin;
use drogue_device::{
    actors::button::{ButtonEvent, FromButtonEvent},
    traits::lora::*,
    Actor, Address, Inbox,
};
pub enum Command {
    Send,
}

impl<D: LoraDriver> FromButtonEvent<Command> for App<D> {
    fn from(event: ButtonEvent) -> Option<Command> {
        match event {
            ButtonEvent::Pressed => None,
            ButtonEvent::Released => Some(Command::Send),
        }
    }
}

pub struct App<D: LoraDriver> {
    config: LoraConfig,
    driver: Option<D>,
}

impl<D: LoraDriver> App<D> {
    pub fn new(config: LoraConfig) -> Self {
        Self {
            config,
            driver: None,
        }
    }
}

impl<D: LoraDriver> Unpin for App<D> {}

impl<D: LoraDriver> Actor for App<D> {
    type Configuration = D;
    #[rustfmt::skip]
    type Message<'m> where D: 'm = Command;

    fn on_mount(&mut self, _: Address<'static, Self>, config: Self::Configuration) {
        self.driver.replace(config);
    }

    #[rustfmt::skip]
    type OnStartFuture<'m, M> where D: 'm, M: 'm = impl Future<Output = ()> + 'm;
    fn on_start<'m, M>(self: Pin<&'m mut Self>, inbox: &'m mut M) -> Self::OnStartFuture<'m, M>
    where
        M: Inbox<'m, Self> + 'm,
    {
        let me = unsafe { self.get_unchecked_mut() };
        async move {
            let driver = me.driver.as_mut().unwrap();
            log::info!("Configuring modem");
            driver.configure(&me.config).await.unwrap();
            log::info!("Joining LoRaWAN network");
            driver.join(ConnectMode::OTAA).await.unwrap();
            log::info!("LoRaWAN network joined");
            let driver = me.driver.as_mut().unwrap();
            loop {
                match inbox.next().await {
                    Some((m, r)) => r.respond(match m {
                        Command::Send => {
                            log::info!("Sending data..");
                            let result = driver.send(QoS::Confirmed, 1, b"ping").await;
                            log::info!("Data sent: {:?}", result);
                        }
                    }),
                    _ => {}
                }
            }
        }
    }
}
