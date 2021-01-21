use crate::actor::{Actor, ActorContext};
use core::cell::UnsafeCell;
use crate::handler::{RequestHandler, NotificationHandler};
use crate::sink::Sink;

pub struct Address<A: Actor> {
    actor: UnsafeCell<*const ActorContext<A>>,
}

impl<A: Actor> Clone for Address<A> {
    fn clone(&self) -> Self {
        Self {
            actor: unsafe { UnsafeCell::new(*self.actor.get()) }
        }
    }
}

// TODO critical sections around ask/tell
impl<A: Actor> Address<A> {
    pub(crate) fn new(actor: &ActorContext<A>) -> Self {
        Self {
            actor: UnsafeCell::new(actor),
        }
    }

    pub fn notify<M>(&self, message: M)
        where A: NotificationHandler<M> + 'static,
              M: 'static
    {
        log::info!("addr::notify");
        unsafe {
            (&**self.actor.get()).notify(message);
        }
        log::info!("addr::notify done");
    }

    pub async fn request<M>(&self, message: M) -> <A as RequestHandler<M>>::Response
        where A: RequestHandler<M> + 'static,
              M: 'static
    {
        unsafe {
            (&**self.actor.get()).request(message).await
        }
    }
}

impl<A: Actor + 'static, M: 'static> Sink<M> for Address<A>
    where A: NotificationHandler<M>
{
    fn notify(&self, message: M) {
        Address::notify(self, message)
    }
}
