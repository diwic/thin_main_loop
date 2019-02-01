
use futures::future::{Future};
use futures::task;
use std::pin::Pin;
use std::sync::Arc;
use crate::MainLoopError;

struct Foo {}

impl task::Wake for Foo {
    fn wake(_: &Arc<Self>) {
        println!("wake!");
    }
}

pub fn spawn<F>(future: F) -> Result<(), MainLoopError>
where F: Future<Output = ()> + Unpin + 'static
{
    let mut fbox = Box::new(future);
    //let fobj: LocalFutureObj<()> = Box::new(future).into();
    crate::call_asap(move || {
        let fpin = Pin::new(&mut fbox); 
        let waker = Foo {};
        let lwaker = task::local_waker_from_nonlocal(Arc::new(waker));
        let r = Future::poll(fpin, &lwaker);
        println!("{:?}", r);
    })
}

