use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct CarveLimiter {
    limit: Option<u64>,
    reserved: AtomicU64,
    carved: AtomicU64,
}

impl CarveLimiter {
    pub fn new(limit: Option<u64>) -> Self {
        Self {
            limit,
            reserved: AtomicU64::new(0),
            carved: AtomicU64::new(0),
        }
    }

    pub fn limit(&self) -> Option<u64> {
        self.limit
    }

    pub fn carved(&self) -> u64 {
        self.carved.load(Ordering::Relaxed)
    }

    pub fn carved_counter(&self) -> &AtomicU64 {
        &self.carved
    }

    pub fn reserved(&self) -> u64 {
        self.reserved.load(Ordering::Relaxed)
    }

    pub fn try_reserve(&self) -> bool {
        match self.limit {
            None => true,
            Some(limit) => loop {
                let carved = self.carved.load(Ordering::Relaxed);
                let reserved = self.reserved.load(Ordering::Relaxed);
                if carved.saturating_add(reserved) >= limit {
                    return false;
                }
                let next = reserved.saturating_add(1);
                if self
                    .reserved
                    .compare_exchange_weak(reserved, next, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    return true;
                }
            },
        }
    }

    pub fn commit(&self) {
        self.carved.fetch_add(1, Ordering::Relaxed);
        if self.limit.is_some() {
            self.dec_reserved();
        }
    }

    pub fn release(&self) {
        if self.limit.is_some() {
            self.dec_reserved();
        }
    }

    pub fn should_stop(&self) -> bool {
        match self.limit {
            Some(limit) => self.carved.load(Ordering::Relaxed) >= limit,
            None => false,
        }
    }

    fn dec_reserved(&self) {
        debug_assert!(self.reserved.load(Ordering::Relaxed) > 0);
        let _ = self
            .reserved
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |value| {
                value.checked_sub(1)
            });
    }
}

#[cfg(test)]
mod tests {
    use super::CarveLimiter;

    #[test]
    fn unlimited_reservations() {
        let limiter = CarveLimiter::new(None);
        assert!(limiter.try_reserve());
        assert!(limiter.try_reserve());
        limiter.commit();
        limiter.commit();
        assert_eq!(limiter.carved(), 2);
        assert_eq!(limiter.reserved(), 0);
        assert!(!limiter.should_stop());
    }

    #[test]
    fn reserves_and_commits_with_limit() {
        let limiter = CarveLimiter::new(Some(2));
        assert!(limiter.try_reserve());
        assert!(limiter.try_reserve());
        assert!(!limiter.try_reserve());
        limiter.commit();
        limiter.commit();
        assert_eq!(limiter.carved(), 2);
        assert_eq!(limiter.reserved(), 0);
        assert!(limiter.should_stop());
    }

    #[test]
    fn release_frees_reservation() {
        let limiter = CarveLimiter::new(Some(1));
        assert!(limiter.try_reserve());
        assert_eq!(limiter.reserved(), 1);
        limiter.release();
        assert_eq!(limiter.reserved(), 0);
        assert!(limiter.try_reserve());
    }
}
