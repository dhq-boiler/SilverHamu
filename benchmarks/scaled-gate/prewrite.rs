impl MvccPages {
    fn prepare_dirty(
        &self,
        db: &Database,
        count: usize,
        bytes: u64,
    ) -> crate::transactions::Result<(usize, u64)> {
        let capacity = count.min((bytes / PAIR).min(96) as usize);
        let _turn = self.turns.enter();
        let _writer = db.inner.writer.lock().map_err(|_| TxError::Poisoned)?;
        let mut guard = self.disk.lock().map_err(|_| TxError::Poisoned)?;
        let disk = &mut *guard;
        if !self.linear || disk.failed || disk.control.poisoned {
            return Err(TxError::Invalid(
                "prewrite requires a healthy linear backend",
            ));
        }
        let ranges: Vec<_> = disk
            .dirty
            .iter()
            .filter(|k| {
                !disk.prepared.contains_key(*k)
                    && (!disk.delay_invalidated || !disk.deferred.contains(*k))
            })
            .take(capacity.saturating_sub(disk.prepared.len()).min(4))
            .cloned()
            .collect();
        let reader = db.begin_read()?;
        if reader.version() != disk.logical_seq() {
            return Err(TxError::Invalid("prewrite snapshot"));
        }
        let generation = disk
            .control
            .root
            .epoch
            .checked_add(1)
            .ok_or(TxError::Invalid("epoch exhausted"))?;
        let before = disk.control.costs.writes.get("page").copied().unwrap_or(0);
        let mut added = 0;
        if ranges.is_empty() {
            return Ok((0, 0));
        }
        let mut scratch = db.inner.budget.buffer();
        scratch.add(PACK_SCRATCH)?;
        let mut page = LinearPage::new();
        page.combine = self.combine;
        for lo in ranges {
            let hi = disk
                .pages
                .range::<[u8], _>((Excluded(lo.as_slice()), Unbounded))
                .next()
                .map(|(k, _)| k.clone());
            let mut first = None;
            let mut outputs = 0;
            let start_bytes = disk.control.costs.writes.get("page").copied().unwrap_or(0);
            let old_cursor = disk.prepared_cursor;
            let result =
                Self::collect_pages(&reader, &lo, hi.as_deref(), &mut page, &mut |_, body| {
                    outputs += 1;
                    if outputs == 1 {
                        first = Some(save(
                            &mut disk.pool,
                            &mut disk.control.costs,
                            &mut disk.live,
                            &mut disk.prepared_cursor,
                            &disk.control.root,
                            generation,
                            PageSource::Linear(body),
                            disk.reuse_encoded,
                            &mut |s| self.signal(s),
                        )?);
                    }
                    Ok(())
                });
            if result.is_err() || outputs != 1 {
                disk.prepared_cursor = old_cursor;
            }
            let written = disk.control.costs.writes.get("page").copied().unwrap_or(0) - start_bytes;
            if result.is_err() {
                disk.stage_bytes.failed += written;
            }
            result?;
            if outputs > 1 {
                disk.stage_bytes.split += written;
            }
            if outputs == 1 {
                let page = first.unwrap();
                disk.stage_bytes.slots.insert(page.slot, written);
                disk.prepared.insert(lo, page);
                added += 1;
            }
        }
        Ok((
            added,
            disk.control.costs.writes.get("page").copied().unwrap_or(0) - before,
        ))
    }
}
