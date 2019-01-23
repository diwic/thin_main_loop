# A thin main loop for Rust

Because Rust's native GUI story starts with the main loop.

## Goals

 * Callback based, but still ergonomic API
 * Cross-platform
 * Negligible performance overhead for desktop applications
 * Bind to the best backend on each platform
 * Compatible with Futures/await/async, when it settles
 * No extra background threads

## Non-goals

 * Avoiding allocations and virtual dispatch at all costs
 * I/O scalability
 * no_std functionality

## Status

The library has functions for running a callback ASAP (as soon as the main loop gets a chance to run something), after a timeout, or at regular intervals. Sending a callback to another thread is also supported.

Maturity: None. It's a proof-of-concept, to spawn discussion and interest.

Needs nightly Rust due to [Box FnOnce](https://github.com/rust-lang/rust/issues/28796), but hopefully that's on the road towards stabilization soon.

## Supported platforms

Currently:

 * Win32 API (compile with `--features "win32"`)
 * Glib (compile with `--features "glib"`)
 * Rust std (if you don't specify any features)

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

# Background

## Callbacks

Most of the APIs for native GUIs build on callbacks. When a button is clicked, you get a callback. The problem though, is that callbacks aren't really rustic: if you want to access your button object in two different callbacks, you end up having to Rc/RefCell it. And while that isn't the end of the world, people have been experimenting with other, more rustic, API designs.

But that will be the task of another library. If we first make a thin cross platform library that binds to the native GUI apis, we can then experiment with making a more rustic API on top of that. And before we can make windows and buttons, we need to make a main loop which can process events from these objects. Hence this library.

## Comparison with Mio

[Mio](https://crates.io/crates/mio) is also a cross platform main loop, but Mio has quite different design goals which ultimately makes it unsuitable for native GUI applications. Mio's primary use case is highly scalable servers, as such it binds to IOCP/epoll/etc which can take thousands of TCP sockets without problems, but does not integrate well with native APIs for GUI libraries: IOCP threads cannot process Windows messages, and so on. This library binds to PeekMessage/GMainLoop/etc, which makes it suitable for GUI applications with native look and feel.

Also, Mio is better at avoiding allocations, at the cost of being less ergonomic.

## Comparison with Calloop

[Calloop](https://crates.io/crates/calloop) is an event loop with very similar callback-style API to this crate. However, it is built on top of Mio, and so it binds to unsuitable native APIs for native GUI applications.

## Comparison with Winit

[Winit](https://crates.io/crates/winit) includes an event loop, and the crate has the purpose of creating windows. The event loop is not callback based, but enum based (every Event is an enum, which you need to dispatch yourself). Winit's focus is more on getting a window and custom drawing (through OpenGL, Vulcan etc) rather than drawing native GUI widgets, but nonetheless has some common ground with this crate.

## Comparison with IUI / libui

[IUI](https://crates.io/crates/iui) is a Rust binding to libui, which is a cross-platform GUI library written in C. Its event loop offers callbacks, much like this library. In comparison, this library is pure Rust only and binds to native libraries directly, skipping one abstraction level and is therefore easier to build. I also hope that with time this library could offer better Rust integration as well as some more flexibility, being usable for more than pure GUI applications, even if that is the current primary use case.
