use std::hint::black_box;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use rscask::keydir::HashMapKeyDir;
use rscask::storage::FileLogStore;
use rscask::{Bitcask, Options};
use tempfile::TempDir;

const KEY_COUNT: usize = 1_000;
const VALUE_SIZE: usize = 256;

type Db = Bitcask<FileLogStore, HashMapKeyDir>;

struct BenchState {
    _dir: TempDir,
    db: Db,
    key: Vec<u8>,
    value: Vec<u8>,
}

fn setup_state() -> BenchState {
    let dir = TempDir::new().expect("create temp dir");
    let mut db = Bitcask::open_with_options(
        dir.path(),
        Options {
            sync_on_write: false,
            ..Options::default()
        },
    )
    .expect("open bitcask");
    let seed_value = vec![b'x'; VALUE_SIZE];
    for i in 0..KEY_COUNT {
        let key = format!("key_{:08}", i);
        db.put(&key, &seed_value).expect("seed put");
    }
    let key = format!("key_{:08}", KEY_COUNT / 2).into_bytes();
    let value = vec![b'y'; VALUE_SIZE];
    BenchState {
        _dir: dir,
        db,
        key,
        value,
    }
}

fn bitcask_get(c: &mut Criterion) {
    c.bench_function("bitcask_get", |b| {
        b.iter_batched(
            setup_state,
            |state| {
                black_box(state.db.get(&state.key).expect("get"));
            },
            BatchSize::LargeInput,
        );
    });
}

fn bitcask_update(c: &mut Criterion) {
    c.bench_function("bitcask_update", |b| {
        b.iter_batched(
            setup_state,
            |mut state| {
                state
                    .db
                    .put(&state.key, &state.value)
                    .expect("update put");
            },
            BatchSize::LargeInput,
        );
    });
}

fn bitcask_delete(c: &mut Criterion) {
    c.bench_function("bitcask_delete", |b| {
        b.iter_batched(
            setup_state,
            |mut state| {
                black_box( state.db.delete(&state.key).expect("delete"));
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, bitcask_get, bitcask_update, bitcask_delete);
criterion_main!(benches);
