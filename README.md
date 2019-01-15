## Cross platform thin main loop

Because Rust's native GUI story starts with the main loop.

## Goals

 * Callback based, but still ergonomic API
 * Cross-platform
 * Negligible performance overhead for desktop applications
 * Bind to the best backend on each platform

## Non-goals

 * Avoiding allocations and virtual dispatch at all costs
 * I/O scalability
 * no_std functionality

## Callbacks

The APIs for native GUI seem to have one thing in common: they build on callbacks. When a button is clicked, you get a callback. The problem though, is that callbacks aren't really rustic: if you want to access your button object in two different callbacks, you end up having to Rc/RefCell it. And while that isn't the end of the world, people have been experimenting with other, more rustic, API designs.

But that will be the task of another library. If we first make a thin cross platform library that binds to the native GUI apis, we can then experiment with making a more rustic API on top of that. And before we can make windows and buttons, we need to make a main loop which can process events from these objects. Hence this library.

## Comparison with Mio

[Mio](https://crates.io/crates/mio) is also a cross platform main loop, but Mio has quite different design goals which ultimately makes it unsuitable for native GUI applications. Mio's primary use case is highly scalable servers, as such it binds to IOCP/epoll/etc which can take thousands of TCP sockets without problems, but does not integrate well with native APIs for GUI libraries: IOCP threads cannot process Windows messages, and so on. This library binds to PeekMessage/GMainLoop/etc, which makes it suitable for GUI applications with native look and feel.

Also, Mio is better at avoiding allocations, at the cost of being less ergonomic.

## Supported platforms

Currently:

 * Win32 API (default on windows)
 * Glib (default on unix)
 * Rust std (fallback other platforms)

Wishlist:

 * OS X
 * Wasm
 * QT
 * iOS
 * Android

