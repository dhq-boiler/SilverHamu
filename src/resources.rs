use crate::transactions::{Error, Result};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    pub max_transactions: usize,
    pub max_staged_bytes: usize,
    pub max_dependency_bytes: usize,
    pub max_retained_bytes: usize,
    pub max_buffer_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_transactions: 64,
            max_staged_bytes: 32 * 1024 * 1024,
            max_dependency_bytes: 16 * 1024 * 1024,
            max_retained_bytes: 128 * 1024 * 1024,
            max_buffer_bytes: 16 * 1024 * 1024,
        }
    }
}

impl ResourceLimits {
    pub fn unlimited() -> Self {
        Self {
            max_transactions: usize::MAX,
            max_staged_bytes: usize::MAX,
            max_dependency_bytes: usize::MAX,
            max_retained_bytes: usize::MAX,
            max_buffer_bytes: usize::MAX,
        }
    }

    pub(crate) fn values(self) -> [usize; 5] {
        [
            self.max_transactions,
            self.max_staged_bytes,
            self.max_dependency_bytes,
            self.max_retained_bytes,
            self.max_buffer_bytes,
        ]
    }

    pub(crate) fn validate(self) -> Result<Self> {
        if self.values().contains(&0) {
            return Err(Error::Invalid("zero resource limit"));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceUsage {
    pub current: [usize; 5],
    pub peak: [usize; 5],
    pub limits: ResourceLimits,
}

pub(crate) const ACTIVE: usize = 0;
pub(crate) const STAGED: usize = 1;
pub(crate) const DEPENDENCIES: usize = 2;
pub(crate) const RETAINED: usize = 3;
pub(crate) const BUFFER: usize = 4;

pub(crate) struct Budget {
    limits: ResourceLimits,
    usage: Mutex<([usize; 5], [usize; 5])>,
}

impl Budget {
    pub fn new(limits: ResourceLimits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            usage: Mutex::new(([0; 5], [0; 5])),
        })
    }

    pub fn replace(&self, kind: usize, old: usize, new: usize) -> Result<()> {
        let mut usage = self.usage.lock().map_err(|_| Error::Poisoned)?;
        let next = usage.0[kind]
            .checked_sub(old)
            .and_then(|value| value.checked_add(new))
            .filter(|value| *value <= self.limits.values()[kind])
            .ok_or(Error::ResourceExhausted("database resource budget"))?;
        usage.0[kind] = next;
        usage.1[kind] = usage.1[kind].max(next);
        Ok(())
    }

    pub fn check_add(&self, kind: usize, bytes: usize) -> Result<()> {
        let usage = self.usage.lock().map_err(|_| Error::Poisoned)?;
        usage.0[kind]
            .checked_add(bytes)
            .filter(|value| *value <= self.limits.values()[kind])
            .ok_or(Error::ResourceExhausted("database resource budget"))?;
        Ok(())
    }

    pub fn release(&self, kind: usize, bytes: usize) {
        // Recovery must still release reservations after a prior operation poisoned a mutex.
        let mut usage = self.usage.lock().unwrap_or_else(|error| error.into_inner());
        usage.0[kind] -= bytes;
    }

    pub fn snapshot(&self) -> Result<ResourceUsage> {
        let usage = self.usage.lock().map_err(|_| Error::Poisoned)?;
        Ok(ResourceUsage {
            current: usage.0,
            peak: usage.1,
            limits: self.limits,
        })
    }

    pub fn buffer_limit(&self) -> usize {
        self.limits.max_buffer_bytes
    }
    pub fn buffer(self: &Arc<Self>) -> BufferPermit {
        BufferPermit {
            budget: self.clone(),
            bytes: 0,
        }
    }
}

pub(crate) struct BufferPermit {
    budget: Arc<Budget>,
    bytes: usize,
}
impl std::fmt::Debug for BufferPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferPermit")
            .field("bytes", &self.bytes)
            .finish()
    }
}
impl BufferPermit {
    pub fn add(&mut self, bytes: usize) -> Result<()> {
        self.budget.replace(BUFFER, 0, bytes)?;
        self.bytes += bytes;
        Ok(())
    }
    pub fn clear(&mut self) {
        self.budget.release(BUFFER, self.bytes);
        self.bytes = 0;
    }
}
impl Drop for BufferPermit {
    fn drop(&mut self) {
        self.budget.release(BUFFER, self.bytes);
    }
}
