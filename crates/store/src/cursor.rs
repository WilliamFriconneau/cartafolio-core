use crate::{ListQuery, StoreError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

#[derive(Serialize, Deserialize)]
pub(crate) struct Cursor {
    pub(crate) snapshot: String,
    pub(crate) last: String,
    pub(crate) at: i64,
}

pub(crate) struct CursorCodec<'a> {
    key: &'a [u8],
    scope: Vec<u8>,
}

impl<'a> CursorCodec<'a> {
    pub(crate) fn new(key: &'a [u8], kind: &str, query: &ListQuery) -> Result<Self, StoreError> {
        let mut filters = query.clone();
        filters.cursor = None;
        for set in [
            &mut filters.spaces,
            &mut filters.tags_any,
            &mut filters.tags_all,
        ] {
            set.sort_unstable();
            set.dedup();
        }
        let scope = serde_json::to_vec(&(kind, filters)).map_err(|_| invalid())?;
        Ok(Self { key, scope })
    }

    pub(crate) fn decode(&self, value: &str) -> Result<Cursor, StoreError> {
        if value.len() > 512 {
            return Err(invalid());
        }
        let mut bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| invalid())?;
        let (signature, payload) = bytes.split_at_mut_checked(32).ok_or_else(invalid)?;
        self.mask(signature, payload)?;
        let mut mac = self.mac(b"signature")?;
        mac.update(payload);
        mac.verify_slice(signature).map_err(|_| invalid())?;
        serde_json::from_slice(payload).map_err(|_| invalid())
    }

    pub(crate) fn encode(&self, cursor: &Cursor) -> Result<String, StoreError> {
        // Both anchors are random IDs of visible rows. Rowids stay inside SQLite.
        let mut payload = serde_json::to_vec(cursor).map_err(|_| invalid())?;
        let mut mac = self.mac(b"signature")?;
        mac.update(&payload);
        let mut signed = mac.finalize().into_bytes().to_vec();
        self.mask(&signed, &mut payload)?;
        signed.extend(payload);
        Ok(URL_SAFE_NO_PAD.encode(signed))
    }

    // The authenticated plaintext determines the IV, so issuing the same
    // cursor is deterministic without reusing a keystream for distinct pages.
    fn mask(&self, signature: &[u8], payload: &mut [u8]) -> Result<(), StoreError> {
        let mut stream = self.mac(b"keystream")?;
        stream.update(signature);
        for (index, chunk) in payload.chunks_mut(32).enumerate() {
            let mut block = stream.clone();
            block.update(&(index as u64).to_be_bytes());
            for (byte, mask) in chunk.iter_mut().zip(block.finalize().into_bytes()) {
                *byte ^= mask;
            }
        }
        Ok(())
    }

    fn mac(&self, purpose: &[u8]) -> Result<Hmac<Sha256>, StoreError> {
        let mut mac = Hmac::new_from_slice(self.key).map_err(|_| invalid())?;
        mac.update(b"pensine-list-cursor-v1");
        mac.update(&(self.scope.len() as u64).to_be_bytes());
        mac.update(&self.scope);
        mac.update(purpose);
        Ok(mac)
    }
}

fn invalid() -> StoreError {
    StoreError::Invalid("Invalid list cursor".into())
}
