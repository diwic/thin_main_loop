//! A thin main loop library for desktop applications and async I/O.
//!
//! See README.md for an introduction and some examples.

#![cfg_attr(feature = "futures", feature(async_await))]

// Because not all backends use everything in the common code
#![allow(unused_variables)]
// #![allow(unused_imports)]
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

/// Possible error codes returned from the main loop API.
#[derive(Debug)]
pub enum MainLoopError {
    TooManyMainLoops,
    NoMainLoop,
    Unsupported,
    DurationTooLong,
    Other(Box<dyn std::error::Error>),
}

/// Callback Id, can be used to cancel callback before its run.
#[derive(Clone, Debug, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CbId(u64);

/// Abstraction around unix fds and windows sockets.
#[cfg(windows)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct CbHandle(pub std::os::windows::io::RawSocket);

/// Abstraction around unix fds and windows sockets.
#[cfg(unix)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct CbHandle(pub std::os::unix::io::RawFd);

/// Abstraction around unix fds and windows sockets.
#[cfg(not(any(windows, unix)))]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct CbHandle(pub i32);

/*
struct CbFuture<'a> {
    #[cfg(feature = "futures")]
    future: Box<futures::Future<Output=()> + Unpin + 'a>,
    #[cfg(not(feature = "futures"))]
    future: &'a (),
    instant: Option<Instant>,
    handle: Option<(CbHandle, IODirection)>,
}
*/

enum CbKind<'a> {
    Asap(Box<dyn FnOnce() + 'a>),
    After(Box<dyn FnOnce() + 'a>, Duration),
    Interval(Box<dyn FnMut() -> bool + 'a>, Duration),
    IO(Box<dyn IOAble + 'a>),
//    Future(CbFuture<'a>),
}

impl<'a> CbKind<'a> {
    // Constructors
    pub fn asap<F: FnOnce() + 'a>(f: F) -> Self { CbKind::Asap(Box::new(f)) }
    pub fn after<F: FnOnce() + 'a>(f: F, d: Duration) -> Self { CbKind::After(Box::new(f), d) }
    pub fn interval<F: FnMut() -> bool + 'a>(f: F, d: Duration) -> Self { CbKind::Interval(Box::new(f), d) }
    pub fn io<IO: IOAble + 'a>(io: IO) -> Self { CbKind::IO(Box::new(io)) }

    // Used to figure out which one it is
    pub fn duration(&self) -> Option<Duration> {
        match self {
            CbKind::IO(_) => None,
            CbKind::Asap(_) => None,
            CbKind::After(_, d) => Some(*d),
            CbKind::Interval(_, d) => Some(*d),
//            CbKind::Future(f) => f.instant.map(|x| x - Instant::now()),
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

    pub fn handle(&self) -> Option<(CbHandle, IODirection)> {
        match self {
            CbKind::IO(io) => Some((io.handle(), io.direction())),
            CbKind::Asap(_) => None,
            CbKind::After(_, _) => None,
            CbKind::Interval(_, _) => None,
//            CbKind::Future(f) => f.handle,
        }
    }

    // If "false" is returned, please continue with making a call to post_call_mut.
    pub (crate) fn call_mut(&mut self, io_dir: Option<Result<IODirection, std::io::Error>>) -> bool {
        match self {
            CbKind::Interval(f, _) => f(),
            CbKind::IO(io) => io.on_rw(io_dir.unwrap()),
            CbKind::After(_, _) => false,
            CbKind::Asap(_) => false,
/*            CbKind::Future(f) => {
                #[cfg(feature = "futures")]
                {
                    future_impl::do_poll(f)
                }
                #[cfg(not(feature = "futures"))]
                unreachable!()
            } */      
        }
    }

    pub (crate) fn post_call_mut(self) {
        match self {
            CbKind::After(f, _) => f(),
            CbKind::Asap(f) => f(),
            CbKind::Interval(_, _) => {},
            CbKind::IO(_) => {},
//            CbKind::Future(_) => {},
        }
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

/// Selects whether to wait for a CbHandle to be available for reading, writing, or both.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum IODirection {
    None,
    Read,
    Write,
    Both,
}

/// Represents an object that can be read from and/or written to.
pub trait IOAble {
    fn handle(&self) -> CbHandle;

    fn direction(&self) -> IODirection;

    fn on_rw(&mut self, _: Result<IODirection, std::io::Error>) -> bool;
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
    fn handle(&self) -> CbHandle { CbHandle(self.io.as_raw_fd()) }

    fn direction(&self) -> IODirection { IODirection::Read }
    fn on_rw(&mut self, r: Result<IODirection, std::io::Error>) -> bool {
        (self.f)(&mut self.io, r);
        true
    }
}

#[cfg(windows)]
impl<IO, F> IOAble for IOReader<IO, F>
where IO: std::os::windows::io::AsRawSocket,
      F: FnMut(&mut IO, Result<IODirection, std::io::Error>)
{
    fn handle(&self) -> CbHandle { CbHandle(self.io.as_raw_socket()) }

    fn direction(&self) -> IODirection { IODirection::Read }
    fn on_rw(&mut self, r: Result<IODirection, std::io::Error>) -> bool {
        (self.f)(&mut self.io, r);
        true
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

#[cfg(feature = "futures")]
pub mod future;

