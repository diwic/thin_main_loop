# A thin main loop for Rust

Because Rust's native GUI story starts with the main loop.

(Although this library might be useful for many other use cases too.)

## Goals

 * Callback based, but still ergonomic API
 * Cross-platform
 * Negligible performance overhead for desktop applications
 * Bind to the best backend on each platform
 * Compatible with Futures/await/async, when it settles
 * No extra background threads
 * Provide access to raw handles to allow platform specific extensions

## Non-goals

 * Avoiding allocations and virtual dispatch at all costs
 * I/O scalability
 * no_std functionality

## Status

The library has functions for running a callback:
 * ASAP (as soon as the main loop gets a chance to run something),
 * after a timeout,
 * at regular intervals,
 * ASAP, but in another thread,
 * when an I/O object is ready of reading or writing.

Maturity: Just up and running, not battle-tested. It's also a proof-of-concept, to spawn discussion and interest.

Unsafe blocks: Only at the backend/FFI level. With the reference (Rust std) backend, there is no unsafe code at all.

Rust version: Latest stable should be fine.

## Supported platforms

Currently:

 * Win32 API - compile with `--features "win32"`
 * Glib - compile with `--features "glib"`
 * Rust std - reference implementation, does not support I/O.

Wishlist:

 * OS X / Cocoa
 * Wasm / web (limited as we don't control the main loop)
 * QT
 * iOS
 * Android

# Examples

## Borrowing

If you have access to the mainloop, it supports borrowing closures so you don't have to clone/refcell your data:

```rust
// extern crate thin_main_loop as tml;

let mut x = false;
{
    let mut ml = tml::MainLoop::new();
    ml.call_asap(|| {
        x = true; // x is mutably borrowed by the closure
        tml::terminate();
    });
    ml.run();
}
assert_eq!(x, true);
```

## Non-borrowing, and a timer

If you don't have access to the mainloop, you can still schedule `'static` callbacks:

```rust
// extern crate thin_main_loop as tml;

let mut ml = tml::MainLoop::new();
ml.call_asap(|| {
    // Inside a callback we can schedule another callback.
    tml::call_after(Duration::new(1, 0), || {
        tml::terminate();
    });
})
ml.run();
// After one second, the main loop is terminated.
```

## I/O

The following example connects to a TCP server and prints everything coming in.

```rust
// extern crate thin_main_loop as tml;

let mut io = TcpStream::connect(/* ..select server here.. */)?;
io.set_nonblocking(true)?;
let wr = tml::IOReader { io: io, f: move |io: &mut TcpStream, x| {
    // On incoming data, read it all
    let mut s = String::new();
    let r = io.read_to_string(&mut s);

    // If we got something, print it
    if s != "" { println!(s); }

    // This is TcpStream's way of saying "connection closed"
    if let Ok(0) = r { tml::terminate(); }
}

let mut ml = MainLoop::new()?;
ml.call_io(wr)?;
ml.run();


```

# Background

## Callbacks

Most of the APIs for native GUIs build on callbacks. When a button is clicked, you get a callback. The problem though, is that callbacks aren't really rustic: if you want to access your button object in two different callbacks, you end up having to Rc/RefCell it. And while that isn't the end of the world, people have been experimenting with other, more rustic, API designs.

But that will be the task of another library. If we first make a thin cross platform library that binds to the native GUI apis, we can then experiment with making a more rustic API on top of that. And before we can make windows and buttons, we need to make a main loop which can process events from these objects. Hence this library.

# Other Rust main loops

## Mio

[Mio](https://crates.io/crates/mio) is also a cross platform main loop, but Mio has quite different design goals which ultimately makes it unsuitable for native GUI applications. Mio's primary use case is highly scalable servers, as such it binds to IOCP/epoll/etc which can take thousands of TCP sockets without problems, but does not integrate well with native APIs for GUI libraries: IOCP threads cannot process Windows messages, and so on. This library binds to [PeekMessage](https://docs.microsoft.com/en-us/windows/desktop/api/winuser/nf-winuser-peekmessagew)/[GMainLoop](https://developer.gnome.org/glib/stable/glib-The-Main-Event-Loop.html)/etc, which makes it suitable for GUI applications with native look and feel.

Also, Mio is better at avoiding allocations, at the cost of being less ergonomic.

## Calloop

[Calloop](https://crates.io/crates/calloop) is an event loop with very similar callback-style API to this crate. However, it is built on top of Mio, and so it binds to unsuitable native APIs for native GUI applications.

## Winit

[Winit](https://crates.io/crates/winit) includes an event loop, and the crate has the purpose of creating windows. The event loop is not callback based, but enum based (every Event is an enum, which you need to dispatch yourself). Winit's focus is more on getting a window and custom drawing (through OpenGL, Vulcan etc) rather than drawing native GUI widgets, but nonetheless has some common ground with this crate.

## IUI / libui

[IUI](https://crates.io/crates/iui) is a Rust binding to libui, which is a cross-platform GUI library written in C. Its event loop offers callbacks, much like this library. In comparison, this library is pure Rust only and binds to native libraries directly, skipping one abstraction level and is therefore easier to build. I also hope that with time this library could offer better Rust integration as well as some more flexibility, being usable for more than pure GUI applications, even if that is the current primary use case.
