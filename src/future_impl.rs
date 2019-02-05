
use futures::future::{Future};
use futures::task;
use std::pin::Pin;
use std::sync::Arc;
use crate::{MainLoopError, CbFuture, CbKind};

struct Foo {}

impl task::Wake for Foo {
    fn wake(_: &Arc<Self>) {
        println!("wake!");
    }
}

pub fn spawn<F>(future: F) -> Result<(), MainLoopError>
where F: Future<Output = ()> + Unpin + 'static
{
    let cbfuture = CbFuture {
        future: Box::new(future),
        instant: None,
        handle: None,
    };
    crate::call_internal(CbKind::Future(cbfuture)).map(|_| ())
}

pub (crate) fn do_poll(f: &mut CbFuture) -> bool {
    let pinfuture = Pin::new(&mut *f.future);
    let waker = task::local_waker_from_nonlocal(Arc::new(Foo {}));
    pinfuture.poll(&waker) == futures::Poll::Pending
}

