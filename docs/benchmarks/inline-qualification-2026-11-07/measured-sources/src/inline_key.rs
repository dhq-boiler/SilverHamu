use std::{borrow::Borrow, cmp::Ordering, ops::Deref};

#[derive(Clone, Debug)]
pub(crate) enum Key {
    Inline { len: u8, bytes: [u8; 15] },
    Heap(Box<[u8]>),
}
impl From<Vec<u8>> for Key {
    fn from(value: Vec<u8>) -> Self {
        if value.len() <= 15 {
            let mut bytes = [0; 15];
            bytes[..value.len()].copy_from_slice(&value);
            Self::Inline {
                len: value.len() as u8,
                bytes,
            }
        } else {
            Self::Heap(value.into_boxed_slice())
        }
    }
}
impl Key {
    #[cfg(feature = "history-metrics")]
    pub(crate) fn capacity(&self) -> usize {
        match self {
            Self::Inline { .. } => 0,
            Self::Heap(bytes) => bytes.len(),
        }
    }
    pub(crate) fn from_slice(value: &[u8]) -> Self {
        if value.len() <= 15 {
            let mut bytes = [0; 15];
            bytes[..value.len()].copy_from_slice(value);
            Self::Inline {
                len: value.len() as u8,
                bytes,
            }
        } else {
            Self::Heap(value.into())
        }
    }
    pub(crate) fn as_slice(&self) -> &[u8] {
        match self {
            Self::Inline { len, bytes } => &bytes[..usize::from(*len)],
            Self::Heap(bytes) => bytes,
        }
    }
}
impl Deref for Key {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}
impl Borrow<[u8]> for Key {
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn bv_layout_and_storage_boundary() {
        assert!(std::mem::size_of::<Key>() <= std::mem::size_of::<Vec<u8>>());
        for n in [0, 1, 8, 14, 15, 16, 17, 4096] {
            let bytes = vec![0; n];
            let key = Key::from_slice(&bytes);
            assert_eq!(key.as_slice(), bytes);
            assert_eq!(matches!(key, Key::Inline { .. }), n <= 15);
            assert_eq!(key.clone(), key);
        }
    }
    #[test]
    fn eq_order_and_borrowed_ranges_match_bytes() {
        let mut keys = vec![vec![], vec![0], vec![0, 0], vec![0, 255], vec![255]];
        for n in [14, 15, 16, 17, 4096] {
            keys.push(vec![1; n]);
            keys.push(vec![255; n]);
        }
        let expected: BTreeMap<_, _> = keys
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, k)| (k, i))
            .collect();
        let actual: BTreeMap<_, _> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (Key::from_slice(k), i))
            .collect();
        assert_eq!(
            actual
                .iter()
                .map(|(k, v)| (k.to_vec(), *v))
                .collect::<Vec<_>>(),
            expected.into_iter().collect::<Vec<_>>()
        );
        for k in keys {
            assert!(actual.get(k.as_slice()).is_some());
        }
    }
}
