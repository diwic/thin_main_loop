#[cfg(feature = "glib")]
use crate::glib::Backend;

#[cfg(feature = "win32")]
use crate::winmsg::Backend;

#[cfg(not(any(feature = "win32", feature = "glib")))]
use crate::ruststd::Backend;

use std::cell::Cell;
use std::ptr::NonNull;
use std::marker::PhantomData;
use std::rc::Rc;
use std::panic;
use std::time::Duration;
use std::sync::Mutex;
use std::collections::HashMap;
use std::thread::ThreadId;
use crate::{CbKind, CbId, MainLoopError, IOAble};

pub (crate) fn call_internal(cb: CbKind<'static>) -> Result<CbId, MainLoopError> {
    current_loop.with(|ml| {
        let ml = ml.get().ok_or(MainLoopError::NoMainLoop)?;
        let ml = unsafe { ml.as_ref() };
        ml.backend.push(cb)
    })
}

pub (crate) trait SendFnOnce: Send {
    fn send(&self, f: Box<FnOnce() + Send + 'static>) -> Result<(), MainLoopError>;
}

lazy_static! {
    static ref THREAD_SENDER: Mutex<HashMap<ThreadId, Box<SendFnOnce>>> = Default::default();
}


pub (crate) fn call_thread_internal(thread: ThreadId, f: Box<FnOnce() + Send + 'static>) -> Result<(), MainLoopError> {
    let map = THREAD_SENDER.lock().unwrap();
    let sender = map.get(&thread).ok_or(MainLoopError::NoMainLoop)?;
    sender.send(f)
}

pub (crate) fn terminate() {
    current_loop.with(|ml| {
        if let Some(ml) = ml.get() { 
            let ml = unsafe { ml.as_ref() };
            ml.quit(); 
        }
    });
}

thread_local! {
    static current_loop: Cell<Option<NonNull<MainLoop<'static>>>> = Default::default();
}



pub struct MainLoop<'a> {
    terminated: Cell<bool>,
    backend: Backend<'a>,
    _z: PhantomData<Rc<()>>, // !Send, !Sync
}

impl<'a> MainLoop<'a> {
    pub fn quit(&self) { self.terminated.set(true) }
    pub fn call_asap<F: FnOnce() + 'a>(&self, f: F) -> Result<CbId, MainLoopError> {
        self.backend.push(CbKind::asap(f))
    }
    pub fn call_after<F: FnOnce() + 'a>(&self, d: Duration, f: F) -> Result<CbId, MainLoopError> { 
        self.backend.push(CbKind::after(f, d))
    }
    pub fn call_interval<F: FnMut() -> bool + 'a>(&self, d: Duration, f: F)  -> Result<CbId, MainLoopError> {
        self.backend.push(CbKind::interval(f, d))
    }
    pub fn call_io<IO: IOAble + 'a>(&self, io: IO)  -> Result<CbId, MainLoopError> {
        self.backend.push(CbKind::io(io))
    }

    fn with_current_loop<F: FnOnce()>(&self, f: F) {
        if self.terminated.get() { return; }
        current_loop.with(|ml| {
            if ml.get().is_some() { panic!("Reentrant call to MainLoop") }
            ml.set(Some(NonNull::from(self).cast()));
        });
        let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
             f()
        }));
        current_loop.with(|ml| { ml.set(None); });
        if let Err(e) = r { panic::resume_unwind(e) };
    }

    /// Runs the main loop until terminated.
    pub fn run(&mut self) {
        self.with_current_loop(|| {
            while !self.terminated.get() {
                self.backend.run_one(true);
            }
        })
    }

    /// Runs the main loop once, without waiting.
    pub fn run_one(&mut self) {
        self.with_current_loop(|| {
            if !self.terminated.get() {
                self.backend.run_one(false);
            }
        })
    }

    /// Creates a new main loop
    pub fn new() -> Result<Self, MainLoopError> {
        let (be, sender) = Backend::new()?;
        let thread_id = std::thread::current().id();
        {
            let mut s = THREAD_SENDER.lock().unwrap();
            if s.contains_key(&thread_id) { return Err(MainLoopError::TooManyMainLoops) };
            s.insert(thread_id, sender);
        }
        Ok(MainLoop { 
            terminated: Cell::new(false),
            backend: be,
            _z: PhantomData 
        })
    }
}

impl Drop for MainLoop<'_> {
    fn drop(&mut self) {
        let thread_id = std::thread::current().id();
        THREAD_SENDER.lock().unwrap().remove(&thread_id);
    }
}

#[test]
fn borrowed() {
    let mut x;
    {
        let mut ml = MainLoop::new().unwrap();
        x = false;
        ml.call_asap(|| { x = true; terminate(); }).unwrap();
        ml.run();
    }
    assert_eq!(x, true);
}

#[test]
fn asap_static() {
    use std::rc::Rc;

    let x;
    let mut ml = MainLoop::new().unwrap();
    x = Rc::new(Cell::new(0));
    let xcl = x.clone();
    ml.call_asap(|| { 
        assert_eq!(x.get(), 0);
        x.set(1);
        crate::call_asap(move || {
            assert_eq!(xcl.get(), 1);
            xcl.set(2);
            terminate();
        }).unwrap();
    }).unwrap();
    ml.run();
    assert_eq!(x.get(), 2);
}

#[test]
fn after() {
    use std::time::Instant;
    let x;
    let mut ml = MainLoop::new().unwrap();
    x = Cell::new(false);
    let n = Instant::now();
    ml.call_after(Duration::from_millis(300), || { x.set(true); terminate(); }).unwrap();
    ml.run();
    assert_eq!(x.get(), true);
    assert!(Instant::now() - n >= Duration::from_millis(300)); 
}

#[test]
fn interval() {
    use std::time::Instant;
    let mut x = 0;
    let mut y = 0;
    let n = Instant::now();
    {
        let mut ml = MainLoop::new().unwrap();
        ml.call_interval(Duration::from_millis(150), || {
            y += 1;
            false
        }).unwrap();
        ml.call_interval(Duration::from_millis(100), || {
           println!("{}", x);
           x += 1;
           if x >= 4 { terminate(); }
           true
        }).unwrap();
        ml.run();
    }
    assert_eq!(y, 1);
    assert_eq!(x, 4);
    assert!(Instant::now() - n >= Duration::from_millis(400)); 
}

#[test]
fn thread_test() {
    use std::thread;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let mut ml = MainLoop::new().unwrap();
    let id = thread::current().id();
    let x = Arc::new(AtomicUsize::new(0));
    let xcl = x.clone();
    thread::spawn(move || {
        let srcid = thread::current().id();
        crate::call_thread(id, move || {
            assert_eq!(id, thread::current().id());
            assert!(id != srcid);
            // println!("Received");
            xcl.store(1, Ordering::SeqCst);
            terminate();
        }).unwrap();
        // println!("Sent");
    });
    ml.run();
    assert_eq!(x.load(Ordering::SeqCst), 1);
}

#[test]
fn io_test() {
    use std::net::TcpStream;
    use std::io::{Write, Read};
    use crate::IODirection;

    // Let's first make a blocking call.
    let mut io = TcpStream::connect("example.com:80").unwrap();
    io.write(b"GET /someinvalidurl HTTP/1.0\n\n").unwrap();
    let mut reply1 = String::new();
    io.read_to_string(&mut reply1).unwrap();
    println!("{}", reply1);

    struct Wrapper(TcpStream, String, String);
    use std::os::unix::io::{RawFd, AsRawFd};
    impl IOAble for Wrapper {
        fn fd(&self) -> RawFd { self.0.as_raw_fd() }
        fn on_rw(&mut self, x: Result<IODirection, std::io::Error>) {
            println!("on_rw: {:?}", x);
            let r = self.0.read_to_string(&mut self.1);
            println!("r = {:?}, len = {}", r, self.1.len());
            if let Ok(n) = r {
                if n == 0 {
                     println!("{}", self.1);
                     assert_eq!(self.1, self.2);
                     terminate();     
                }
            }
        }
    }

    // And now the non-blocking call.
    let mut ml = MainLoop::new().unwrap();
    let mut io = TcpStream::connect("example.com:80").unwrap();
    io.set_nonblocking(true).unwrap();
    io.write(b"GET /someinvalidurl HTTP/1.0\n\n").unwrap();
    ml.call_io(Wrapper(io, String::new(), reply1)).unwrap();
    ml.run();
}

