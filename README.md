# Keep Calm (and call Clone)

![Build Status](https://github.com/mmastrac/keepcalm/actions/workflows/rust.yml/badge.svg)
[![docs.rs](https://docs.rs/keepcalm/badge.svg)](https://docs.rs/keepcalm)
[![crates.io](https://img.shields.io/crates/v/keepcalm.svg)](https://crates.io/crates/keepcalm)

Simple shared types for multi-threaded Rust programs.

This library simplifies a number of shared-object patterns that are used in multi-threaded programs such as web-servers.

Advantages of `keepcalm`:

 * You don't need to decide on your synchronization primitives up-front. Everything is a [`Shared`] or [`SharedMut`], no matter whether it's
 a mutex, read/write lock, read/copy/update primitive, or a read-only shared [`std::sync::Arc`].
 * Everything is [`project!`]able, which means you can adjust the granularity of your locks at any time without having to refactor the whole
 system. If you want finer-grained locks at a later date, the code that uses the shared containers doesn't change!
 * Writeable containers can be turned into read-only containers, while still retaining the ability for other code to update the contents.

## Container types

The following container types are available:

| Container                      | Equivalent            | Notes |
|--------------------------------|-----------------------|-------|
| SharedMut (RwLock)             | `Arc<RwLock<T>>`      | This is the default shared-mutable type.
| SharedMut (Mutex)              | `Arc<Mutex<T>>`       | In some cases it may be necessary to serialize both read and writes. For example, with types that are not `Sync`.
| SharedMut (read/copy/update)   | `Arc<RwLock<Arc<T>`   | When the write lock of an RCU container is dropped, the values written are committed to the value in the container.
| Shared (default)               | `Arc`                 | This is the default shared-immutable type. Note that this is slightly more verbose: [`Shared`] does not [`std::ops::Deref`] to the underlying type and requires calling [`Shared::read`].
| Shared ([`Shared::new_unsync`])| `Arc<Mutex<T>>`       | For types that are not `Sync`, a `Mutex` is used to serialize read-only access.
| Shared ([`SharedMut::shared`]) | n/a                   | This provides a read-only view into a read-write container and has no direct equivalent.

## Basic syntax

The traditional Rust shared object patterns tend to be somewhat verbose, for example:

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

We can reduce a some of the ceremony and verbosity with `keepcalm`:

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

## SharedMut

The [`SharedMut`] object hides the complexity of managing `Arc<Mutex<T>>` or `Arc<RwLock<T>>` behind a single interface:

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
let shared = SharedMut::new_with_type("123".to_string(), Implementation::Mutex);
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

The [`Shared`] object is similar to Rust's [`std::sync::Arc`], but adds the ability to project.

## Projection

Both [`Shared`] and [`SharedMut`] allow *projection* into the underlying type. Projection can be used to select
either a subset of a type, or to cast a type to a trait.

Note that projections are always linked to the root object!

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
