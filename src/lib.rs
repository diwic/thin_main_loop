//! A thin main loop library for desktop applications and async I/O.
//!
//! See README.md for an introduction.

#![feature(unsized_locals)]

#[cfg(feature = "glib")]
mod glib;

#[cfg(feature = "win32")]
mod winmsg;

#[cfg(feature = "web")]
mod web;

#[cfg(not(any(feature = "win32", feature = "glib", feature = "web")))]
mod ruststd;

#[cfg(not(feature = "web"))]
mod mainloop;

#[cfg(not(feature = "web"))]
pub use mainloop::MainLoop;

use std::time::Duration;

// TODO: Threads (call something on another thread)
// TODO: Cancel callbacks before they are run
// TODO: Futures integration

// pub struct CbId(u32);

/// Possible error codes returned from the main loop API.
#[derive(Copy, Clone, Debug)]
pub enum MainLoopError {
    TooManyMainLoops,
    NoMainLoop,
    DurationTooLong,
}

/// Callback Id, can be used to cancel callback before its run.
///
/// The cancel function is not implemented yet.
#[derive(Clone, Debug)]
pub struct CbId();

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
    pub fn duration_millis(&self) -> Result<Option<u32>, MainLoopError> {
        if let Some(d) = self.duration() {
            let m = (u32::max_value() / 1000) - 1;
            let s = d.as_secs();
            if s >= m as u64 { return Err(MainLoopError::DurationTooLong) }
            Ok(Some((s as u32) * 1000 + d.subsec_millis()))
        } else { Ok(None) } 
    }
}

fn call_internal(cb: CbKind<'static>) -> Result<CbId, MainLoopError> { 
    #[cfg(not(feature = "web"))]
    let r = mainloop::call_internal(cb);

    #[cfg(feature = "web")]
    let r = web::call_internal(cb);
    r
}


/// Runs a function as soon as possible, i e, when the main loop runs.
///
/// Corresponding platform specific APIs:
/// * glib: g_idle_add
/// * node.js: process.nextTick
/// * web: Promise.resolve().then(...)
/// * win32: PostMessage
pub fn call_asap<F: FnOnce() + 'static>(f: F) -> Result<CbId, MainLoopError> {
    let cb = CbKind::asap(f);
    call_internal(cb)
}

/// Runs a function once, after a specified duration.
///
/// Corresponding platform specific APIs:
/// * glib: g_timeout_add
/// * node.js: setTimeout
/// * web: window.setTimeout
/// * win32: SetTimer
pub fn call_after<F: FnOnce() + 'static>(d: Duration, f: F) -> Result<CbId, MainLoopError> {
    let cb = CbKind::after(f, d);
    call_internal(cb)
}

/// Runs a function at regular intervals
///
/// Return "true" from the function to continue running or "false" to
/// remove the callback from the main loop.
///
/// Corresponding platform specific APIs:
/// * glib: g_timeout_add
/// * node.js: setInterval
/// * web: window.setInterval
/// * win32: SetTimer
pub fn call_interval<F: FnMut() -> bool + 'static>(d: Duration, f: F) -> Result<CbId, MainLoopError> {
    let cb = CbKind::interval(f, d);
    call_internal(cb)
}

/// Terminates the currently running main loop.
///
/// This function does nothing if the main loop is not running.
/// This function does nothing with the "web" feature.
pub fn terminate() {
    #[cfg(not(feature = "web"))]
    mainloop::terminate();
}


