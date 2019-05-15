//! 0.3 Futures support (requires the "futures" feature).

use futures::future::{Future};
use futures::task;
use futures::stream::Stream;
use futures::task::{Poll, Waker, Context, ArcWake};
use std::pin::Pin;
use std::mem;
use std::sync::{Arc, Mutex};
use crate::{MainLoopError, MainLoop, IODirection, CbHandle, IOAble};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::cell::{Cell, RefCell};

use std::time::Instant;

/// Waits until a specific instant.
pub struct Delay(Instant);

impl Future for Delay {
    type Output = Result<(), MainLoopError>;
    fn poll(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Self::Output> {
        let n = Instant::now();
        // println!("Polled at {:?}", n);
        if self.0 <= n { Poll::Ready(Ok(())) }
        else {
            let lw = ctx.waker().clone();
            match crate::call_after(self.0 - n, move || { lw.wake() }) {
                Ok(_) => Poll::Pending,
                Err(e) => Poll::Ready(Err(e)),
            }
        }
    }
}

/// Waits until a specific instant.
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

/// Io implements "futures::Stream", so it will output an item whenever 
/// the handle is ready for read / write.
pub struct Io(Rc<IoInternal>);

impl IOAble for Io {
    fn handle(&self) -> CbHandle { self.0.cb_handle }
    fn direction(&self) -> IODirection { self.0.direction }
    fn on_rw(&mut self, r: Result<IODirection, std::io::Error>) -> bool {
        self.0.queue.borrow_mut().push_back(r);
        let w = self.0.waker.borrow();
        if let Some(waker) = &*w { waker.wake_by_ref() };
        self.0.alive.get()
    }
}

impl Stream for Io {
    type Item = Result<IODirection, MainLoopError>;
    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Option<Self::Item>> {
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
            *s.waker.borrow_mut() = Some(ctx.waker().clone());
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

/// Creates a new Io, which outputs an item whenever the handle is ready for reading / writing. 
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
    fn wake_by_ref(x: &Arc<Self>) {
        x.1.lock().unwrap().push(x.0);
        // println!("Waking up");
    }
}

/// A futures executor that supports spawning futures. 
///
/// If you use "Delay" or "Io", this is the executor you need to
/// spawn it on.
/// It contains a MainLoop inside, so you can spawn 'static callbacks too. 
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

    /// Runs until the main loop is terminated.
    pub fn run(&mut self) {
        while self.run_one(true) {}
    }

    /// Processes futures ready to make progress.
    ///
    /// If no futures are ready to progress, may block in case allow_wait is true.
    /// Returns false if the mainloop was terminated.
    pub fn run_one(&mut self, allow_wait: bool) -> bool {
        let run_queue: Vec<_> = {
            let mut r = self.run_queue.lock().unwrap();
            mem::replace(&mut *r, vec!())
        };
        if run_queue.len() == 0 {
            return self.ml.run_one(allow_wait);
        }
        for id in run_queue {
            let remove = {
                let f = self.tasks.get_mut(&id);
                if let Some(f) = f {
                    let pinf = f.as_mut();
                    let t = Task(id, self.run_queue.clone());
                    let t = Arc::new(t);
                    let waker = task::waker_ref(&t);
                    let mut ctx = Context::from_waker(&waker);
                    pinf.poll(&mut ctx) != futures::Poll::Pending
                } else { false }
            };
            if remove {
                self.tasks.remove(&id);
            }
        }
        true
    }

    /// Runs until the future is ready, or the main loop is terminated.
    ///
    /// Returns None if the main loop is terminated, or the result of the future otherwise.
    pub fn block_on<R: 'a, F: Future<Output=R> + 'a>(&mut self, f: F) -> Option<R> {
        use futures::future::{FutureExt, ready};
        let res = Arc::new(RefCell::new(None));
        let res2 = res.clone();
        let f = f.then(move |r| { *res2.borrow_mut() = Some(r); ready(()) });
        self.spawn(f);
        loop {
            if !self.run_one(true) { return None };
            let x = res.borrow_mut().take();
            if x.is_some() { return x; }
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
        delay(n).await.unwrap();
    }

    let mut x = Executor::new().unwrap();
    let n = Instant::now() + Duration::from_millis(200);
    x.block_on(foo(n));
    assert!(Instant::now() >= n);
}
