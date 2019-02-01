#[cfg(feature = "glib")]
use crate::glib::Backend;

#[cfg(feature = "win32")]
use crate::winmsg::Backend;

#[cfg(not(any(feature = "win32", feature = "glib")))]
use crate::ruststd::Backend;

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;
use std::panic;
use std::any::Any;
use std::time::Duration;
use std::sync::Mutex;
use std::collections::HashMap;
use std::thread::ThreadId;
use crate::{CbKind, CbId, MainLoopError, IOAble};
use boxfnonce::SendBoxFnOnce;


#[derive(Default)]
struct MlTls {
    exists: Cell<bool>,
    terminated: Cell<bool>,
    running: Cell<bool>,
    in_queue: RefCell<Vec<CbKind<'static>>>,
    current_panic: RefCell<Option<Box<dyn Any + Send + 'static>>>,
}

// Panic handling

thread_local! {
    static ml_tls: MlTls = Default::default();
}

pub (crate) fn ffi_cb_wrapper<R, F: FnOnce() -> R>(f: F, on_panic: R) -> R {
    match panic::catch_unwind(panic::AssertUnwindSafe(|| { f() })) {
        Ok(x) => x,
        Err(e) => {
            ml_tls.with(|m| {
                // We should never get a double panic, but if we do, let's ignore the info from the second one.
                // Probably the info from the first one is the more helpful.
                let _ = m.current_panic.try_borrow_mut().map(|mut cp| { *cp = Some(e); });
            });
            on_panic
        }
    }
}

// Thread sends

pub (crate) trait SendFnOnce: Send {
    fn send(&self, f: SendBoxFnOnce<'static, ()>) -> Result<(), MainLoopError>;
}

lazy_static! {
    static ref THREAD_SENDER: Mutex<HashMap<ThreadId, Box<SendFnOnce>>> = Default::default();
}

pub (crate) fn call_thread_internal(thread: ThreadId, f: SendBoxFnOnce<'static, ()>) -> Result<(), MainLoopError> {
    let map = THREAD_SENDER.lock().unwrap();
    let sender = map.get(&thread).ok_or(MainLoopError::NoMainLoop)?;
    sender.send(f)
}



pub (crate) fn call_internal(cb: CbKind<'static>) -> Result<(), MainLoopError> {
    ml_tls.with(|m| {
        if !m.exists.get() { return Err(MainLoopError::NoMainLoop) }
        m.in_queue.borrow_mut().push(cb);
        Ok(())
    })
}

pub (crate) fn terminate() {
    ml_tls.with(|m| {
        m.terminated.set(true);
    });
}

pub struct MainLoop<'a> {
    backend: Backend<'a>,
    _z: PhantomData<Rc<()>>, // !Send, !Sync
}

impl<'a> MainLoop<'a> {
    pub fn terminate(&self) { terminate() }
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

    fn run_wrapper<F: FnOnce()>(&self, f: F) -> bool {
        ml_tls.with(|m| {
            if m.terminated.get() { return false; }
            {
                let mut q = m.in_queue.borrow_mut();
                for cbk in q.drain(..) {
                    self.backend.push(cbk).unwrap(); // TODO: Should probably be reported better
                }
            }
            if m.running.get() { panic!("Reentrant call to MainLoop") }
            m.running.set(true);
            f();
            m.running.set(false);
            if let Some(e) = m.current_panic.borrow_mut().take() {
                panic::resume_unwind(e);
            }
            true
        })
    }

    /// Runs the main loop until terminated.
    pub fn run(&mut self) {
        while self.run_wrapper(|| {
            self.backend.run_one(true);
        }) {}
    }

    /// Runs the main loop once, without waiting.
    pub fn run_one(&mut self) {
        self.run_wrapper(|| {
            self.backend.run_one(false);
        });
    }

    /// Creates a new main loop
    pub fn new() -> Result<Self, MainLoopError> {
        ml_tls.with(|m| {
            if m.exists.get() { return Err(MainLoopError::TooManyMainLoops) };

            let (be, sender) = Backend::new()?;
            let thread_id = std::thread::current().id();
            {
                let mut s = THREAD_SENDER.lock().unwrap();
                if s.contains_key(&thread_id) { return Err(MainLoopError::TooManyMainLoops) };
                s.insert(thread_id, sender);
            }

            m.in_queue.borrow_mut().clear();
            m.current_panic.borrow_mut().take();
            m.terminated.set(false);
            m.running.set(false);
            m.exists.set(true);

            Ok(MainLoop { 
                backend: be,
                _z: PhantomData 
            })
        })
    }
}

impl Drop for MainLoop<'_> {
    fn drop(&mut self) {
        ml_tls.with(|m| { m.exists.set(false); });
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

#[cfg(any(feature = "glib", feature = "win32"))]
#[test]
fn io_test() {
    use std::net::TcpStream;
    use std::io::{Write, Read};
    use crate::IOReader;

    // Let's first make a blocking call.
    let mut io = TcpStream::connect("example.com:80").unwrap();
    io.write(b"GET /someinvalidurl HTTP/1.0\r\n\r\n").unwrap();
    let mut reply1 = String::new();
    io.read_to_string(&mut reply1).unwrap();
    println!("{}", reply1);

    // And now the non-blocking call.
    let mut ml = MainLoop::new().unwrap();
    let mut io = TcpStream::connect("example.com:80").unwrap();
    io.set_nonblocking(true).unwrap();
    io.write(b"GET /someinvalidurl HTTP/1.0\r\n\r\n").unwrap();

    let mut reply2 = String::new();
    let wr = IOReader { io: io, f: move |io: &mut TcpStream, x| {
        println!("{:?}", x);
        // assert_eq!(x.unwrap(), IODirection::Read);
        let r = io.read_to_string(&mut reply2);
        println!("r = {:?}, len = {}", r, reply2.len());
        if let Ok(n) = r {
            if n == 0 {
                 println!("{}", reply2);
                 // Skip the headers, they contain a date field that causes spurious failures
                 let r1: Vec<_> = reply1.split("\r\n\r\n").collect();
                 let r2: Vec<_> = reply2.split("\r\n\r\n").collect();
                 assert_eq!(r1.len(), r2.len());
                 assert!(r2.len() > 1);
                 assert_eq!(r1[1], r2[1]);
                 terminate();
            }
        }
    }};
    ml.call_io(wr).unwrap();
    ml.run();
}

#[test]
fn panic_inside_cb() {
    let mut ml = MainLoop::new().unwrap();
    ml.call_asap(|| { panic!("Keep calm and carry on"); }).unwrap();
    let z = panic::catch_unwind(panic::AssertUnwindSafe(|| { ml.run(); }));
    let z = z.unwrap_err();
    let zstr = z.downcast_ref::<&str>().unwrap();
    assert_eq!(*zstr, "Keep calm and carry on");
}

