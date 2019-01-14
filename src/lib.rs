//! A thin main loop library for desktop applications and async I/O.
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

// mod ruststd;
// use ruststd::Backend;

// #![windows_subsystem = "windows"]

#[cfg(unix)]
mod glib;
#[cfg(unix)]
use crate::glib::Backend;

#[cfg(windows)]
mod winmsg;
#[cfg(windows)]
use crate::winmsg::Backend;

use std::time::Duration;

use std::cell::Cell;
use std::ptr::NonNull;
use std::marker::PhantomData;
use std::rc::Rc;
use std::panic;

// pub struct CbId(u32);

enum CbKind<'a> {
    Asap(Box<dyn FnOnce() + 'a>),
    After(Box<dyn FnOnce() + 'a>, Duration),
    Interval(Box<dyn FnMut() -> bool + 'a>, Duration),
}

impl<'a> CbKind<'a> {
    pub fn asap<F: FnOnce() + 'a>(f: F) -> Self { CbKind::Asap(Box::new(f)) }
    pub fn after<F: FnOnce() + 'a>(f: F, d: Duration) -> Self { CbKind::After(Box::new(f), d) }
    pub fn interval<F: FnMut() -> bool + 'a>(f: F, d: Duration) -> Self { CbKind::Interval(Box::new(f), d) }
    pub fn duration(&self) -> Option<Duration> {
        match self {
            CbKind::Asap(_) => None,
            CbKind::After(_, d) => Some(*d),
            CbKind::Interval(_, d) => Some(*d),
        }
    }
}

pub struct MainLoop<'a> {
    terminated: Cell<bool>,
    backend: Backend<'a>,
    _z: PhantomData<Rc<()>>,
}

impl<'a> MainLoop<'a> {
    pub fn quit(&self) { self.terminated.set(true) }
    pub fn call_asap<F: FnOnce() + 'a>(&self, f: F) { self.backend.push(CbKind::asap(f)) } 
    pub fn call_after<F: FnOnce() + 'a>(&self, d: Duration, f: F) { self.backend.push(CbKind::after(f, d)) }
    pub fn call_interval<F: FnMut() -> bool + 'a>(&self, d: Duration, f: F) { self.backend.push(CbKind::interval(f, d)) }

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
    pub fn new() -> Self { MainLoop { 
        terminated: Cell::new(false),
        backend: Backend::new(),
        _z: PhantomData 
    } }
}

fn call_internal(cb: CbKind<'static>) {
    current_loop.with(|ml| {
        let ml = ml.get().unwrap();
        let ml = unsafe { ml.as_ref() };
        ml.backend.push(cb);
    });
}

/// Runs a function as soon as possible, i e, when the main loop runs.
///
/// Corresponding platform specific APIs:
/// * Glib: g_idle_add
/// * Node.js: process.nextTick
/// * Browser: Promise.resolve().then(...)
/// * Windows: PostMessage
pub fn call_asap<F: FnOnce() + 'static>(f: F) {
    let cb = CbKind::asap(f);
    call_internal(cb);
}

/// Runs a function once, after a specified duration.
///
/// Corresponding platform specific APIs:
/// * Glib: g_timeout_add
/// * Node.js: setTimeout
/// * Browser: window.setTimeout
/// * Windows: SetTimer
pub fn call_after<F: FnOnce() + 'static>(d: Duration, f: F) {
    let cb = CbKind::after(f, d);
    call_internal(cb);
}

/// Runs a function at regular intervals
///
/// Return "true" from the function to continue running or "false" to
/// remove the callback from the main loop.
///
/// Corresponding platform specific APIs:
/// * Glib: g_timeout_add
/// * Node.js: setInterval
/// * Browser: window.setInterval
/// * Windows: SetTimer
pub fn call_interval<F: FnMut() -> bool + 'static>(d: Duration, f: F) {
    let cb = CbKind::interval(f, d);
    call_internal(cb);
}

/// Terminates the currently running main loop.
///
/// This function does nothing if the main loop is not running.
pub fn terminate() {
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
    ml.call_asap(|| { x.set(true); terminate(); });
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
    ml.call_asap(|| { 
        assert_eq!(x.get(), 0);
        x.set(1);
        call_asap(move || {
            assert_eq!(xcl.get(), 1);
            xcl.set(2);
            terminate();
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
    ml.call_after(Duration::from_millis(300), || { x.set(true); terminate(); });
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
        ml.call_interval(Duration::from_millis(150), || {
            y += 1;
            false
        });
        ml.call_interval(Duration::from_millis(100), || {
           println!("{}", x);
           x += 1;
           if x >= 4 { terminate(); }
           true
        });
        ml.run();
    }
    assert_eq!(y, 1);
    assert_eq!(x, 4);
    assert!(Instant::now() - n >= Duration::from_millis(400)); 
}
