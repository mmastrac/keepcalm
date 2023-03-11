use criterion::{criterion_group, criterion_main, Criterion};
use keepcalm::{make_spawner, Shared, Spawner};

static SPAWNER: Spawner = make_spawner!(tokio::task::spawn_blocking);

async fn acquire_lock_tokio(lock: &tokio::sync::Mutex<usize>) -> usize {
    *lock.lock().await
}

async fn acquire_rwlock_tokio(lock: &tokio::sync::RwLock<usize>) -> usize {
    *lock.read().await
}

async fn acquire_lock_keepcalm(lock: &Shared<usize>) -> usize {
    *lock.read_async(&SPAWNER).await
}

async fn acquire_lock_keepcalm_noasync(lock: &Shared<usize>) -> usize {
    *lock.read()
}

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("tokio mutex", |b| {
        let lock = tokio::sync::Mutex::new(1);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        b.to_async(runtime).iter(|| acquire_lock_tokio(&lock));
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

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
