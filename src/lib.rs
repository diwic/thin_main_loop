//! A thin main loop library for desktop applications and async I/O.
//!
//! See README.md for an introduction.

// Because Box<FnOnce>
#![feature(unsized_locals)]

// Because this is just an unfinished prototype
#![allow(unused_variables)]
#![allow(dead_code)]

#[macro_use]
extern crate lazy_static;

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
pub use crate::mainloop::MainLoop;

use std::time::Duration;
use std::thread::ThreadId;

// TODO: Threads (call something on another thread)
// TODO: Cancel callbacks before they are run
// TODO: Futures integration

// pub struct CbId(u32);

/// Possible error codes returned from the main loop API.
#[derive(Debug)]
pub enum MainLoopError {
    TooManyMainLoops,
    NoMainLoop,
    Unsupported,
    DurationTooLong,
    Other(Box<std::error::Error>),
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
    IO(Box<IOAble + 'a>),
}

impl<'a> CbKind<'a> {
    pub fn asap<F: FnOnce() + 'a>(f: F) -> Self { CbKind::Asap(Box::new(f)) }
    pub fn after<F: FnOnce() + 'a>(f: F, d: Duration) -> Self { CbKind::After(Box::new(f), d) }
    pub fn interval<F: FnMut() -> bool + 'a>(f: F, d: Duration) -> Self { CbKind::Interval(Box::new(f), d) }
    pub fn io<IO: IOAble + 'a>(io: IO) -> Self { CbKind::IO(Box::new(io)) }
    pub fn duration(&self) -> Option<Duration> {
        match self {
            CbKind::IO(_) => None,
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

fn call_internal(cb: CbKind<'static>) -> Result<(), MainLoopError> { 
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
pub fn call_asap<F: FnOnce() + 'static>(f: F) -> Result<(), MainLoopError> {
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
pub fn call_after<F: FnOnce() + 'static>(d: Duration, f: F) -> Result<(), MainLoopError> {
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
pub fn call_interval<F: FnMut() -> bool + 'static>(d: Duration, f: F) -> Result<(), MainLoopError> {
    let cb = CbKind::interval(f, d);
    call_internal(cb)
}


/// Runs a function on another thread. The target thread must run a main loop.
#[cfg(not(feature = "web"))]
pub fn call_thread<F: FnOnce() + Send + 'static>(thread: ThreadId, f: F) -> Result<(), MainLoopError> {
    mainloop::call_thread_internal(thread, Box::new(f)) 
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum IODirection {
    None,
    Read,
    Write,
    Both,
}

/// Represents an object that can be read from and/or written to.
pub trait IOAble {
    #[cfg(unix)]
    fn fd(&self) -> std::os::unix::io::RawFd;
    #[cfg(windows)]
    fn socket(&self) -> std::os::windows::io::RawSocket;

    fn direction(&self) -> IODirection;

    fn on_rw(&mut self, _: Result<IODirection, std::io::Error>) {}
    /* TODO: Handle Errors / hangup / etc */
}

/// The most common I/O object is one from which you can read asynchronously.
/// This is a simple convenience wrapper for that kind of I/O object.
pub struct IOReader<IO, F: FnMut(&mut IO, Result<IODirection, std::io::Error>)>{
    pub io: IO,
    pub f: F,
}

#[cfg(unix)]
impl<IO, F> IOAble for IOReader<IO, F>
where IO: std::os::unix::io::AsRawFd,
      F: FnMut(&mut IO, Result<IODirection, std::io::Error>)
{
    fn fd(&self) -> std::os::unix::io::RawFd { self.io.as_raw_fd() }

    fn direction(&self) -> IODirection { IODirection::Read }
    fn on_rw(&mut self, r: Result<IODirection, std::io::Error>) {
        (self.f)(&mut self.io, r)
    }
}

#[cfg(windows)]
impl<IO, F> IOAble for IOReader<IO, F>
where IO: std::os::windows::io::AsRawSocket,
      F: FnMut(&mut IO, Result<IODirection, std::io::Error>)
{
    fn socket(&self) -> std::os::windows::io::RawSocket { self.io.as_raw_socket() }

    fn direction(&self) -> IODirection { IODirection::Read }
    fn on_rw(&mut self, r: Result<IODirection, std::io::Error>) {
        (self.f)(&mut self.io, r)
    }
}

/// Calls IOAble's callbacks when there is data to be read or written.
pub fn call_io<IO: IOAble + 'static>(io: IO) -> Result<(), MainLoopError> {
    let cb = CbKind::io(io);
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


