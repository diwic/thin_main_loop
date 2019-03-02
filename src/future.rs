
use futures::future::{Future};
use futures::task;
use futures::stream::Stream;
use futures::task::{Poll, Waker, ArcWake};
use std::pin::Pin;
use std::mem;
use std::sync::{Arc, Mutex};
use crate::{MainLoopError, MainLoop, IODirection, CbHandle, IOAble};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::cell::{Cell, RefCell};

use std::time::Instant;

pub struct Delay(Instant);

impl Future for Delay {
    type Output = Result<(), MainLoopError>;
    fn poll(self: Pin<&mut Self>, lw: &Waker) -> Poll<Self::Output> {
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

struct IoInternal {
    cb_handle: CbHandle,
    direction: IODirection,
    queue: RefCell<VecDeque<Result<IODirection, std::io::Error>>>,
    alive: Cell<bool>,
    started: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

pub struct Io(Rc<IoInternal>);

impl IOAble for Io {
    fn handle(&self) -> CbHandle { self.0.cb_handle }
    fn direction(&self) -> IODirection { self.0.direction }
    fn on_rw(&mut self, r: Result<IODirection, std::io::Error>) -> bool {
        self.0.queue.borrow_mut().push_back(r);
        let w = self.0.waker.borrow();
        if let Some(waker) = &*w { waker.wake() };
        self.0.alive.get()
    }
}

impl Stream for Io {
    type Item = Result<IODirection, MainLoopError>;
    fn poll_next(self: Pin<&mut Self>, lw: &Waker) -> Poll<Option<Self::Item>> {
        let s: &IoInternal = &(*self).0;
        if !s.alive.get() { return Poll::Ready(None); }

        if !s.started.get() {
            // Submit to the reactor
            let c: &Rc<IoInternal> = &(*self).0;
            let c = Io(c.clone());
            if let Err(e) = crate::call_io(c) {
                s.alive.set(false);
                return Poll::Ready(Some(Err(e)));
            }
            s.started.set(true);
        }

        let q = s.queue.borrow_mut().pop_front();
        if let Some(item) = q {
            let item = item.map_err(|e| MainLoopError::Other(Box::new(e)));
            Poll::Ready(Some(item))
        } else {
            *s.waker.borrow_mut() = Some(lw.clone());
            Poll::Pending
        }
    }
}

impl Drop for Io {
    fn drop(&mut self) {
        let s: &IoInternal = &(*self).0;
        s.alive.set(false);
    }
}

pub fn io(handle: CbHandle, dir: IODirection) -> Io {
    Io(Rc::new(IoInternal {
        cb_handle: handle,
        direction: dir,
        alive: Cell::new(true),
        started: Cell::new(false),
        queue: Default::default(),
        waker: Default::default(),
    }))
}

// And the executor stuff 

type BoxFuture<'a> = Pin<Box<Future<Output=()> + 'a>>;

type RunQueue = Arc<Mutex<Vec<u64>>>;

struct Task(u64, RunQueue);

impl ArcWake for Task {
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
                        let t = Arc::new(t);
                        let waker = task::waker_ref(&t);
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

#[test]
fn async_fn_test() {
    use std::time::Duration;

    async fn foo(n: Instant) {
        await!(delay(n)).unwrap();
        crate::terminate();
    }

    let mut x = Executor::new().unwrap();
    let n = Instant::now() + Duration::from_millis(200);
    x.spawn(foo(n));
    x.run();
    assert!(Instant::now() >= n);
}
