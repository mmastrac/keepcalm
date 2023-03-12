use std::{sync::Arc, time::Duration};

use criterion::{criterion_group, criterion_main, Criterion};
use keepcalm::{make_spawner, Shared, Spawner};

static SPAWNER: Spawner = make_spawner!(tokio::task::spawn_blocking);

async fn acquire_lock_tokio(lock: &tokio::sync::Mutex<usize>) -> usize {
    *lock.lock().await
}

async fn acquire_lock_and_sleep_tokio(lock: Arc<tokio::sync::Mutex<usize>>) {
    let lock2 = lock.clone();
    let f = tokio::spawn(async move {
        let l = lock2.lock().await;
        tokio::time::sleep(Duration::from_micros(10)).await;
        drop(l);
    });
    _ = *lock.lock().await;
    _ = f.await;
}

async fn acquire_rwlock_tokio(lock: &tokio::sync::RwLock<usize>) -> usize {
    *lock.read().await
}

async fn acquire_lock_keepcalm(lock: &Shared<usize>) -> usize {
    *lock.read_async(&SPAWNER).await
}

async fn acquire_lock_and_sleep_keepcalm(lock: Shared<usize>) {
    let lock2 = lock.clone();
    let f = tokio::spawn(async move {
        let l = lock2.read();
        tokio::time::sleep(Duration::from_micros(10)).await;
        drop(l);
    });
    _ = *lock.read_async(&SPAWNER).await;
    _ = f.await;
}

async fn acquire_lock_keepcalm_noasync(lock: &Shared<usize>) -> usize {
    *lock.read()
}

fn async_benchmark(c: &mut Criterion) {
    c.bench_function("tokio mutex", |b| {
        let lock = tokio::sync::Mutex::new(1);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        b.to_async(runtime).iter(|| acquire_lock_tokio(&lock));
    });

    c.bench_function("tokio mutex (contended)", |b| {
        let lock = Arc::new(tokio::sync::Mutex::new(1));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        b.to_async(runtime)
            .iter(|| acquire_lock_and_sleep_tokio(lock.clone()));
    });

    c.bench_function("tokio rwlock", |b| {
        let lock = tokio::sync::RwLock::new(1);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        b.to_async(runtime).iter(|| acquire_rwlock_tokio(&lock));
    });

    c.bench_function("keepcalm mutex", |b| {
        let lock = Shared::new_mutex(1);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        b.to_async(runtime).iter(|| acquire_lock_keepcalm(&lock));
    });

    c.bench_function("keepcalm mutex (contended)", |b| {
        let lock = Shared::new_mutex(1);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        b.to_async(runtime)
            .iter(|| acquire_lock_and_sleep_keepcalm(lock.clone()));
    });

    c.bench_function("keepcalm rwlock", |b| {
        let lock = Shared::new(1);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        b.to_async(runtime).iter(|| acquire_lock_keepcalm(&lock));
    });

    c.bench_function("keepcalm rwlock (no async)", |b| {
        let lock = Shared::new(1);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        b.to_async(runtime)
            .iter(|| acquire_lock_keepcalm_noasync(&lock));
    });
}

fn sync_benchmark(c: &mut Criterion) {
    c.bench_function("parking_lot mutex (raw)", |b| {
        let lock = parking_lot::const_mutex(1);
        b.iter(|| lock.lock());
    });

    c.bench_function("keepcalm mutex (raw)", |b| {
        let lock = Shared::new(1);
        b.iter(|| lock.read());
    });
}

criterion_group!(benches, async_benchmark, sync_benchmark);
criterion_main!(benches);
