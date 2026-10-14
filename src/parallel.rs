use std::io;

use rayon::{ThreadPool, ThreadPoolBuilder, prelude::*};

pub(crate) const MAX_THREADS: usize = 64;

pub(crate) enum Executor {
    Sequential,
    Parallel { threads: usize, pool: ThreadPool },
}

impl Executor {
    pub fn new(threads: Option<usize>) -> io::Result<Self> {
        let threads = threads.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1)
                .min(MAX_THREADS)
        });
        if !(1..=MAX_THREADS).contains(&threads) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Число потоков 1..=64",
            ));
        }
        if threads == 1 {
            Ok(Self::Sequential)
        } else {
            let pool = ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .map_err(io::Error::other)?;
            Ok(Self::Parallel { threads, pool })
        }
    }

    pub fn batch_size(&self) -> usize {
        match self {
            Self::Sequential => 1,
            Self::Parallel { threads, .. } => (2 * threads).max(4),
        }
    }

    pub fn map<T: Send, U: Send>(
        &self,
        items: Vec<T>,
        operation: impl Fn(T) -> U + Sync + Send,
    ) -> io::Result<Vec<U>> {
        match self {
            Self::Sequential => Ok(items.into_iter().map(operation).collect()),
            Self::Parallel { pool, .. } if items.len() > 1 => {
                Ok(pool.install(|| items.into_par_iter().map(operation).collect()))
            }
            Self::Parallel { .. } => Ok(items.into_iter().map(operation).collect()),
        }
    }
}
