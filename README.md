# Keep Calm (and call Clone)

Simple shared types for multi-threaded programs

This library simplifies a number of shared-object patterns that are used in multi-threaded programs such as web-servers. The
traditional Rust shared object patterns tend to be somewhat version, for example:

```rust
# use std::sync::{Arc, Mutex};
let object = "123".to_string();
let shared = Arc::new(Mutex::new(object));
shared.lock().expect("Mutex was poisoned");
```

## SharedRW

The [`SharedRW`] object hides the complexity of managing `Arc<Mutex<T>>` or `Arc<RwLock<T>>` behind a single interface:

```rust
# use keepcalm::*;
let object = "123".to_string();
let shared = SharedRW::new(object);
shared.lock_read();
```

By default, a [`SharedRW`] object uses `Arc<RwLock<T>>` under the hood, but you can choose the synchronization primitive at
construction time. The [`SharedRW`] object *erases* the underlying primitive and you can use them interchangeably:

```rust
# use keepcalm::*;
fn use_shared(shared: SharedRW<String>) {
    shared.lock_read();
}

let shared = SharedRW::new("123".to_string());
use_shared(shared);
let shared = SharedRW::new_with_type("123".to_string(), Implementation::Mutex);
use_shared(shared);
```

Managing the poison state of synchronization primitives can be challenging as well. Rust will poison a `Mutex` or `RwLock` if you
hold a lock while a `panic!` occurs.

The `SharedRW` type allows you to specify a [`PoisonPolicy`] at construction time. By default, if a synchronization
primitive is poisoned, the `SharedRW` will `panic!` on access. This can be configured so that poisoning is ignored:

```rust
# use keepcalm::*;
let shared = SharedRW::new_with_policy("123".to_string(), PoisonPolicy::Ignore);
```

## Shared

The [`Shared`] object is similar to Rust's [`Arc`], but adds the ability to project.

## Projection

Both [`Shared`] and [`SharedRW`] allow *projection* into the underlying type. Projection can be used to select
either a subset of a type, or to cast a type to a trait.

Note that projections are always linked to the root object!

Casting:

```rust
# use keepcalm::*;
let shared = SharedRW::new("123".to_string());
let shared_asref: SharedRW<dyn AsRef<str>> = shared.project(project_cast!(x: String => dyn AsRef<str>));
```

Subset of a struct/tuple:

```rust
# use keepcalm::*;
#[derive(Default)]
struct Foo {
    tuple: (String, usize)
}

let shared = SharedRW::new(Foo::default());
let shared_string: SharedRW<String> = shared.project(project!(x: Foo, x.tuple.0));

*shared_string.lock_write() += "hello, world";
assert_eq!(shared.lock_read().tuple.0, "hello, world");
assert_eq!(*shared_string.lock_read(), "hello, world");
```
