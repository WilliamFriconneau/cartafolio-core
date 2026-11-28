use crate::fact::head;
use crate::{Fact, FactStatus, Store, StoreError};

impl Store {
    pub fn pin(&self, id: &str, pinned: bool) -> Result<Fact, StoreError> {
        self.write(|tx| {
            let mut fact = head(tx, id, FactStatus::Active)?;
            if !pinned && crate::todo::is_todo(&fact) {
                return Err(crate::todo::permanent_error());
            }
            // Break clock ties in write order, even across different store handles.
            tx.execute(
                "UPDATE facts SET pinned = ?2, pinned_at = CASE WHEN ?2 THEN
                     max(CAST(unixepoch('subsec') * 1000 AS INTEGER),
                         coalesce((SELECT max(pinned_at) FROM facts WHERE pinned_at IS NOT NULL), 0) + 1)
                 END WHERE id = ?1",
                (id, pinned),
            )?;
            fact.pinned = pinned;
            Ok(fact)
        })
    }
}
