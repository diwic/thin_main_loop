
use futures::future::{Future};
use futures::task;
// use futures::stream::Stream;
use futures::task::{Poll, LocalWaker, Wake};
use std::pin::Pin;
use std::mem;
use std::sync::{Arc, Mutex};
use crate::{MainLoopError, MainLoop};
use std::collections::HashMap;

use std::time::Instant;

pub struct Delay(Instant);

impl Future for Delay {
    type Output = Result<(), MainLoopError>;
    fn poll(self: Pin<&mut Self>, lw: &LocalWaker) -> Poll<Self::Output> {
        let n = Instant::now();
        // println!("Polled at {:?}", n);
        if self.0 <= n { Poll::Ready(Ok(())) }
        else {
            let lw = lw.clone();
            match crate::call_after(self.0 - n, move || { lw.wake() }) {
                Ok(_) => Poll::Pending,
                Err(e) => Poll::Ready(Err(e)),
            }
        }
    }
}

pub fn delay(i: Instant) -> Delay {
    Delay(i)
}
/*
impl<'a, IO: IOAble + 'a> Stream for Io<IO> {
    type Item = Result<IODirection, MainLoopError>;
    fn poll_next(self: Pin<&mut Self>, lw: &LocalWaker) -> Poll<Option<Self::Item>> {
    }
}

pub fn io<IO>(io: IO) -> Io<IO> { Io(io) }


pub struct Io<IO>(IO, Arc<Mutex<Vec<Result<IODirection, std::io::Error>>>>);
*/

// And the executor stuff 

type BoxFuture<'a> = Pin<Box<Future<Output=()> + 'a>>;

type RunQueue = Arc<Mutex<Vec<u64>>>;

struct Task(u64, RunQueue);

impl Wake for Task {
    fn wake(x: &Arc<Self>) {
        x.1.lock().unwrap().push(x.0);
        // println!("Waking up");
    }
}

pub struct Executor<'a> {
    ml: MainLoop<'a>,
    tasks: HashMap<u64, BoxFuture<'a>>,
    next_task: u64,
    run_queue: RunQueue,
}

impl<'a> Executor<'a> {
    pub fn new() -> Result<Self, MainLoopError> {
        Ok(Executor { ml: MainLoop::new()?, next_task: 1, run_queue: Default::default(), tasks: Default::default() })
    }

    pub fn run(&mut self) {
        loop {
            let run_queue: Vec<_> = {
                let mut r = self.run_queue.lock().unwrap();
                mem::replace(&mut *r, vec!())
            };
            // println!("{:?}, Queue: {:?}", Instant::now(), run_queue);
            if run_queue.len() == 0 {
                if !self.ml.run_one(true) { break; } else { continue; }
            }
            for id in run_queue {
                let remove = {
                    let f = self.tasks.get_mut(&id);
                    if let Some(f) = f {
                        let pinf = f.as_mut();
                        let t = Task(id, self.run_queue.clone());
                        let waker = task::local_waker_from_nonlocal(Arc::new(t));
                        pinf.poll(&waker) != futures::Poll::Pending
                    } else { false }
                };
                if remove {
                    self.tasks.remove(&id);
                }
            }
        }
    }

    pub fn spawn<F: Future<Output=()> + 'a>(&mut self, f: F) {
        let x = Box::pin(f);
        self.tasks.insert(self.next_task, x);
        self.run_queue.lock().unwrap().push(self.next_task);
        self.next_task += 1;
    }
}

#[test]
fn delay_test() {
    use std::time::Duration;
    use futures::future::{FutureExt, ready};

    let mut x = Executor::new().unwrap();
    let n = Instant::now() + Duration::from_millis(200);
    let f = delay(n).then(|_| { println!("Terminating!"); crate::terminate(); ready(()) });
    x.spawn(f);
    x.run();
    assert!(Instant::now() >= n);
}
