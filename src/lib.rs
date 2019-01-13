//! A simple main loop library for desktop applications and async I/O.
//!
//! # Goals
//!
//! * Ergonomic API
//! * Cross-platform
//! * Negligible performance overhead (for desktop applications)
//! * Provide the best backend on each platform
//!
//! # Non-goals
//! 
//! * Avoiding allocations and virtual dispatch at all costs
//! * I/O scalability (use mio for this)
//! * no_std functionality

#![feature(unsized_locals)]

mod ruststd;

use ruststd::Backend;

/*
use std::time::Duration;

pub mod io {
    pub trait Able {}
    pub enum Ops { Read, Write, ReadWrite };
}

pub mod run {

/// Runs a function as soon as the main loop runs.
///
/// Corresponding platform specific APIs:
/// * Glib: g_idle_add
/// * Node.js: process.nextTick
/// * Browser: Promise.resolve().then(...)
/// * Windows: PostMessage
pub fn asap<F: FnOnce>(f: F) { unimplemented!() }

/// Runs a function once, after a specified duration.
///
/// Corresponding platform specific APIs:
/// * Glib: g_timeout_add
/// * Node.js: setTimeout
/// * Browser: window.setTimeout
/// * Windows: SetTimer
pub fn after<F: FnOnce>(d: Duration, f: F) { unimplemented!() }

/// Runs a function at regular intervals
///
/// Corresponding platform specific APIs:
/// * Glib: g_timeout_add
/// * Node.js: setInterval
/// * Browser: window.setInterval
/// * Windows: SetTimer
pub fn interval<F: FnMut>(d: Duration, f: F) { unimplemented!() }

/// Runs a function when there is data to read or write
///
/// Corresponding platform specific APIs:
/// * Glib: g_source_add_unix_fd()
/// * Windows: Overlapped I/O
pub fn io<I: io::IOAble, F: FnMut(Result<io::Ops, io::Error>)>(i: I, ops: io::Ops, f: F) { unimplemented!() }

}

pub mod mainloop {

/// Initializes the main loop on the current thread.
pub fn init() { unimplemented!() }

/// Tells all calls to "run" to quit.
pub fn quit() -> bool { unimplemented!() }

/// Runs until quit is called. 
pub fn run() { unimplemented!() }

/// Calls all waiting functions, then returns immediately.
pub fn run_once() { unimplemented!() }

pub fn run_max(d: Duration) { unimplemented!() }

pub fn is_running() -> bool { unimplemented!() }

pub fn can_run() -> bool { unimplemented!() }
}
*/

use std::time::Duration;
use std::cell::RefCell;
use std::collections::VecDeque;	
/*
#[derive(Default)]
struct Native<'a> {
    asap: RefCell<VecDeque<Box<dyn FnOnce(&MainLoop<'a>) + 'a>>>
}

impl<'a> Native<'a> {
    fn add_asap(&self, f: Box<dyn FnOnce(&MainLoop<'a>) + 'a>) { self.asap.borrow_mut().push_back(f) }
    fn run_one(&self, ml: &MainLoop<'a>) -> bool {
         let f = self.asap.borrow_mut().pop_front();
         if let Some(f) = f {
             f(ml);
             return true;
         }
         false
    }
    fn wait(&self) { unimplemented!() }
}
*/
use std::cell::Cell;
use std::ptr::NonNull;
use std::marker::PhantomData;
use std::rc::Rc;
use std::panic;

pub struct CbId(u32);

pub struct MainLoop<'a> {
    terminated: Cell<bool>,
    asap: RefCell<VecDeque<Box<dyn FnOnce(&MainLoop<'a>) + 'a>>>,
    backend: Backend<'a>,
    _z: PhantomData<Rc<()>>,
}

impl<'a> MainLoop<'a> {
    pub fn quit(&self) { self.terminated.set(true) }
    pub fn call_asap<F: FnOnce(&Self) + 'a>(&self, f: F) { self.asap.borrow_mut().push_back(Box::new(f)) }
    pub fn call_after<F: FnOnce(&Self) + 'a>(&self, d: Duration, f: F) { self.backend.push_after(d, Box::new(f)) }
    pub fn call_interval<F: FnMut(&Self) -> bool + 'a>(&self, d: Duration, f: F) { self.backend.push_interval(d, Box::new(f)) }

    fn check_asap(&self) -> bool {
         let f = self.asap.borrow_mut().pop_front();
         if let Some(f) = f {
             f(self);
             true
         } else { false }
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

    pub fn run(&mut self) {
        self.with_current_loop(|| {
            while !self.terminated.get() {
                if self.check_asap() { continue; }
                self.backend.run_one(&self, true);
/*                if !self.backend.run_one(&self) {
                    self.backend.wait()
                } */
            }
        })

/*        if self.terminated.get() { return; }
        current_loop.with(|ml| {
            if ml.get().is_some() { panic!("Reentrant call to MainLoop.run") }
            ml.set(Some(NonNull::from(self).cast()));
        });
        let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            while !self.terminated.get() {
                if !self.backend.run_one(&self) {
                    self.backend.wait()
                }
            }
        }));
        current_loop.with(|ml| { ml.set(None); });
        if let Err(e) = r { panic::resume_unwind(e) }; */
    }
    pub fn new() -> Self { MainLoop { 
        terminated: Cell::new(false),
        asap: Default::default(),
        backend: Backend::new(),
        _z: PhantomData 
    } }
}


pub fn call_asap<F: FnOnce() + 'static>(f: F) {
    current_loop.with(|ml| {
        let ml = ml.get().unwrap();
        let ml = unsafe { ml.as_ref() };
        ml.call_asap(|_| f());
    });
}

pub fn quit() {
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

#[test]
fn borrowed() {
    let x;
    let mut ml = MainLoop::new();
    x = Cell::new(false);
    ml.call_asap(|ml| { x.set(true); ml.quit() });
    ml.run();
    assert_eq!(x.get(), true);
}

#[test]
fn asap_static() {
    use std::rc::Rc;

    let x;
    let mut ml = MainLoop::new();
    x = Rc::new(Cell::new(0));
    let xcl = x.clone();
    ml.call_asap(|_| { 
        assert_eq!(x.get(), 0);
        x.set(1);
        call_asap(move || {
            assert_eq!(xcl.get(), 1);
            xcl.set(2);
            quit();
        })
    });
    ml.run();
    assert_eq!(x.get(), 2);
}

#[test]
fn after() {
    use std::time::Instant;
    let x;
    let mut ml = MainLoop::new();
    x = Cell::new(false);
    let n = Instant::now();
    ml.call_after(Duration::from_millis(300), |ml| { x.set(true); ml.quit() });
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
        let mut ml = MainLoop::new();
        ml.call_interval(Duration::from_millis(50), |_| {
            y += 1;
            assert_eq!(y, 1);
            false
        });
        ml.call_interval(Duration::from_millis(100), |ml| {
           x += 1; 
           if x >= 4 { ml.quit() }
           true
        });
        ml.run();
    }
    assert_eq!(x, 4);
    assert!(Instant::now() - n >= Duration::from_millis(400)); 
}
