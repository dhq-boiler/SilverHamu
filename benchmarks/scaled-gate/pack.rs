impl MvccPages {
    fn pack_range(
        reader: &Transaction,
        lo: &[u8],
        hi: Option<&[u8]>,
        emit: &mut dyn FnMut(Vec<u8>, &WriteSet) -> io::Result<()>,
    ) -> io::Result<()> {
        Self::collect_pages(reader, lo, hi, &mut WriteSet::new(), &mut |start, w| {
            emit(start, w)
        })
    }
    fn collect_pages<T: RowCollector>(
        reader: &Transaction,
        lo: &[u8],
        hi: Option<&[u8]>,
        entries: &mut T,
        emit: &mut dyn FnMut(Vec<u8>, &mut T) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut after: Option<Vec<u8>> = None;
        let mut fence = lo.to_vec();
        let mut bytes = 140usize;
        loop {
            let mut split = None;
            let done = {
                let _phase = Phase::enter(1);
                let state = reader.database.state().map_err(io::Error::other)?;
                let start = after.as_deref().map_or(Included(lo), Excluded);
                let mut rows = state
                    .rows
                    .range::<[u8], _>((start, hi.map_or(Unbounded, Excluded)))
                    .peekable();
                let mut consumed = None;
                for _ in 0..512 {
                    let Some((k, versions)) = rows.next() else {
                        break;
                    };
                    let key = k.as_slice();
                    if let Some(value) = crate::transactions::visible(versions, reader.version())
                        .and_then(|v| v.value.as_deref())
                    {
                        let size = 9 + key.len() + value.len();
                        if 140 + size > BLOCK {
                            return Err(err("row exceeds page"));
                        }
                        if bytes + size > BLOCK || entries.count() == MAX_OPS {
                            split = Some(key.to_vec());
                            break;
                        }
                        entries.push_row(key, value);
                        bytes += size;
                    }
                    consumed = Some(key);
                }
                if let Some(last) = consumed {
                    after = Some(last.to_vec());
                }
                split.is_none() && rows.peek().is_none()
            };
            if split.is_some() || done {
                if entries.count() > 0 {
                    emit(std::mem::take(&mut fence), entries)?;
                    entries.reset();
                    bytes = 140;
                }
            }
            if let Some(next) = split {
                fence = next;
            }
            if done {
                return Ok(());
            }
        }
    }
}
