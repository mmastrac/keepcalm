# Keep Calm (and call Clone)

![Build Status](https://github.com/mmastrac/keepcalm/actions/workflows/rust.yml/badge.svg)
[![docs.rs](https://docs.rs/keepcalm/badge.svg)](https://docs.rs/keepcalm)
[![crates.io](https://img.shields.io/crates/v/keepcalm.svg)](https://crates.io/crates/keepcalm)

Simple shared types for multi-threaded Rust programs: `keepcalm` gives you permission to simplify your synchronization code in concurrent Rust applications.

Name inspired by @luser's [Keep Calm and Call Clone](https://github.com/luser/keep-calm-and-call-clone).

## Overview

This library simplifies a number of shared-object patterns that are used in multi-threaded programs such as web-servers.

Advantages of `keepcalm`:

 * You don't need to decide on your synchronization primitives up-front. Everything is a [`Shared`] or [`SharedMut`], no matter whether it's
 a mutex, read/write lock, read/copy/update primitive, or a read-only shared [`std::sync::Arc`].
 * Everything is [`project!`]able, which means you can adjust the granularity of your locks at any time without having to refactor the whole
 system. If you want finer-grained locks at a later date, the code that uses the shared containers doesn't change!
 * Writeable containers can be turned into read-only containers, while still retaining the ability for other code to update the contents.
 * Read and write guards are `Send` thanks to the `parking_lot` crate.
 * Each synchronization primitive transparently manages the poisoned state (if code `panic!`s while the lock is being held). If you don't want to
 poison on `panic!`, constructors are available to disable this option entirely.
 * `static` Globally-scoped containers for both `Sync` and `!Sync` objects are easily constructed using [`SharedGlobal`], and can provide [`Shared`]
 containers. Mutable global containers can similarly be constructed with [`SharedGlobalMut`].  ***NOTE**: This requires the `--feature global_experimental` flag*

## Container types

The following container types are available:

| Container                      | Equivalent               | Notes |
|--------------------------------|--------------------------|-------|
| [`SharedMut::new`]             | `Arc<RwLock<T>>`         | This is the default shared-mutable type.
| [`SharedMut::new_mutex`]       | `Arc<Mutex<T>>`          | In some cases it may be necessary to serialize both read and writes. For example, with types that are not `Sync`.
| [`SharedMut::new_rcu`]         | `Arc<RwLock<Arc<T>`      | When the write lock of an RCU container is dropped, the values written are committed to the value in the container.
| [`Shared::new`]                | `Arc`                    | This is the default shared-immutable type. Note that this is slightly more verbose: [`Shared`] does not [`std::ops::Deref`] to the underlying type and requires calling [`Shared::read`].
| [`Shared::new_mutex`]          | `Arc<Mutex<T>>`          | For types that are not `Sync`, a `Mutex` is used to serialize read-only access.
| [`SharedMut::shared`]          | n/a                      | This provides a read-only view into a read-write container and has no direct equivalent.

The following global container types are available:

| Container                      | Equivalent               | Notes |
|--------------------------------|--------------------------|-------|
| [`SharedGlobal::new`]          | `static T`               | This is a global `const`-style object, for types that are `Send` + `Sync`.
| [`SharedGlobal::new_lazy`]     | `static Lazy<T>`         | This is a lazily-initialized global `const`-style object, for types that are `Send` + `Sync`.
| [`SharedGlobal::new_mutex`]    | `static Mutex<T>`        | This is a global `const`-style object, for types that are `Send` but not necessarily `Sync`
| [`SharedGlobalMut::new`]       | `static RwLock<T>`       | This is a global mutable object, for types that are `Send` + `Sync`.
| [`SharedGlobalMut::new_lazy`]  | `static Lazy<RwLock<T>>` | This is a lazily-initialized global mutable object, for types that are `Send` + `Sync`.
| [`SharedGlobalMut::new_mutex`] | `static Mutex<T>`        | This is a global mutable object, for types that are `Send` but not necessarily `Sync`.

## Basic syntax

The traditional Rust shared object patterns tend to be somewhat verbose and repetitive, for example:

```rust
# use std::sync::{Arc, Mutex};
# fn use_string(s: &str) {}
struct Foo {
    my_string: Arc<Mutex<String>>,
    my_integer: Arc<Mutex<u16>>,
}
let foo = Foo {
    my_string: Arc::new(Mutex::new("123".to_string())),
    my_integer: Arc::new(Mutex::new(1)),
};
use_string(&*foo.my_string.lock().expect("Mutex was poisoned"));
```

If we want to switch our shared fields from [`std::sync::Mutex`] to [`std::sync::RwLock`], we need to change four lines just for types, and
switch the `lock` method for a `read` method.

We can increase flexibility, and reduce some of the ceremony and verbosity with `keepcalm`:

```rust
# use keepcalm::*;
# fn use_string(s: &str) {}
struct Foo {
    my_string: SharedMut<String>,
    my_integer: SharedMut<u16>,
}
let foo = Foo {
    my_string: SharedMut::new("123".to_string()),
    my_integer: SharedMut::new(1),
};
use_string(&*foo.my_string.read());
```

If we want to use a `Mutex` instead of the default `RwLock` that [`SharedMut`] uses under the hood, we only need to change [`SharedMut::new`] to
[`SharedMut::new_mutex`]! 

## SharedMut

The [`SharedMut`] object hides the complexity of managing `Arc<Mutex<T>>`, `Arc<RwLock<T>>`, and other synchronization types
behind a single interface:

```rust
# use keepcalm::*;
let object = "123".to_string();
let shared = SharedMut::new(object);
shared.read();
```

By default, a [`SharedMut`] object uses `Arc<RwLock<T>>` under the hood, but you can choose the synchronization primitive at
construction time. The [`SharedMut`] object *erases* the underlying primitive and you can use them interchangeably:

```rust
# use keepcalm::*;
fn use_shared(shared: SharedMut<String>) {
    shared.read();
}

let shared = SharedMut::new("123".to_string());
use_shared(shared);
let shared = SharedMut::new_mutex("123".to_string());
use_shared(shared);
```

Managing the poison state of synchronization primitives can be challenging as well. Rust will poison a `Mutex` or `RwLock` if you
hold a lock while a `panic!` occurs.

The `SharedMut` type allows you to specify a [`PoisonPolicy`] at construction time. By default, if a synchronization
primitive is poisoned, the `SharedMut` will `panic!` on access. This can be configured so that poisoning is ignored:

```rust
# use keepcalm::*;
let shared = SharedMut::new_with_policy("123".to_string(), PoisonPolicy::Ignore);
```

## Shared

The default [`Shared`] object is similar to Rust's [`std::sync::Arc`], but adds the ability to project. [`Shared`] objects may also be
constructed as a `Mutex`, or may be a read-only view into a [`SharedMut`].

Note that because of this flexibility, the [`Shared`] object is slightly more complex than a traditional [`std::sync::Arc`], as all accesses
must be performed through the [`Shared::read`] accessor.

## EXPERIMENTAL: Globals

***NOTE**: This requires the `--feature global_experimental` flag*

While `static` globals may often be an anti-pattern in Rust, this library also offers easily-to-use alternatives that are compatible with
the [`Shared`] and [`SharedMut`] types.

Global [`Shared`] references can be created using [`SharedGlobal`]:

```rust
# use keepcalm::*;
# #[cfg(feature="global_experimental")]
static GLOBAL: SharedGlobal<usize> = SharedGlobal::new(1);

# #[cfg(feature="global_experimental")]
fn use_global() {
    assert_eq!(GLOBAL.read(), 1);

    // ... or ...

    let shared: Shared<usize> = GLOBAL.shared();
    assert_eq!(shared.read(), 1);
}
```

Similarly, global [`SharedMut`] references can be created using [`SharedGlobalMut`]:

```rust
# use keepcalm::*;
# #[cfg(feature="global_experimental")]
static GLOBAL: SharedGlobalMut<usize> = SharedGlobalMut::new(1);

# #[cfg(feature="global_experimental")]
fn use_global() {
    *GLOBAL.write() = 12;
    assert_eq!(GLOBAL.read(), 12);

    // ... or ...

    let shared: SharedMut<usize> = GLOBAL.shared_mut();
    *shared.write() = 12;
    assert_eq!(shared.read(), 12);
}
```

Both [`SharedGlobal`] and [`SharedGlobalMut`] offer a `new_lazy` constructor that allows initialization to be deferred to first
access:

```rust
# use keepcalm::*;
# use std::collections::HashMap;
# #[cfg(feature="global_experimental")]
static GLOBAL_LAZY: SharedGlobalMut<HashMap<&str, usize>> =
    SharedGlobalMut::new_lazy(|| HashMap::from_iter([("a", 1), ("b", 2)]));
```

## EXPERIMENTAL: Async

***NOTE**: This requires the `--feature async_experimental` flag*

This is extremely experimental and may have soundness and/or performance issues!

The [`Shared`] and [`SharedMut`] types support a `read_async` and `write_async` method that will block using an async runtime's `spawn_blocking`
method (or equivalent). Create a [`Spawner`] using `make_spawner` and pass that to the appropriate lock method.

```rust
# use keepcalm::*;
# #[cfg(feature="global_experimental")]
static SPAWNER: Spawner = make_spawner!(tokio::task::spawn_blocking);

# #[cfg(feature="global_experimental")]
async fn get_locked_value(shared: Shared<usize>) -> usize {
    *shared.read_async(&SPAWNER).await
}

# #[cfg(feature="global_experimental")]
{
    let shared = Shared::new(1);
    get_locked_value(shared);
}
```


## Projection

Both [`Shared`] and [`SharedMut`] allow *projection* into the underlying type. Projection can be used to select
either a subset of a type, or to cast a type to a trait. The [`project!`] and [`project_cast!`] macros can simplify
this code.

Note that projections are always linked to the root object! If a projection is locked, the root object is locked.

Casting:

```rust
# use keepcalm::*;
let shared = SharedMut::new("123".to_string());
let shared_asref: SharedMut<dyn AsRef<str>> = shared.project(project_cast!(x: String => dyn AsRef<str>));
```

Subset of a struct/tuple:

```rust
# use keepcalm::*;
#[derive(Default)]
struct Foo {
    tuple: (String, usize)
}

let shared = SharedMut::new(Foo::default());
let shared_string: SharedMut<String> = shared.project(project!(x: Foo, x.tuple.0));

*shared_string.write() += "hello, world";
assert_eq!(shared.read().tuple.0, "hello, world");
assert_eq!(*shared_string.read(), "hello, world");
```

## Unsized types

Both [`Shared`] and [`SharedMut`] support unsized types, but due to current limitations in the language (see [`std::ops::CoerceUnsized`] for details),
you need to construct them in special ways.

Unsized traits are supported, but you will either need to specify `Send + Sync` in the shared type, or [`project_cast!`] the object:

```rust
# use keepcalm::*;

// In this form, `Send + Sync` are visible in the shared type
let boxed: Box<dyn AsRef<str> + Send + Sync> = Box::new("123".to_string());
let shared: SharedMut<dyn AsRef<str> + Send + Sync> = SharedMut::from_box(boxed);

// In this form, `Send + Sync` are erased via projection
let shared = SharedMut::new("123".to_string());
let shared_asref: SharedMut<dyn AsRef<str>> = shared.project(project_cast!(x: String => dyn AsRef<str>));
```

Unsized slices are supported using a box:

```rust
# use keepcalm::*;
let boxed: Box<[i32]> = Box::new([1, 2, 3]);
let shared: SharedMut<[i32]> = SharedMut::from_box(boxed);
```
