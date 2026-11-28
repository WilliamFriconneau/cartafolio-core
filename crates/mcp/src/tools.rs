use crate::dispatch::{
    output, FileList, FilePut, List, Note, Recall, Result, Supersede, ToolError,
};
use crate::texts;
use base64::{engine::general_purpose::STANDARD, Engine};
use pensine_access::Principal;
use pensine_store::{
    Fact, FactStatus, FileInfo, Hit, ListQuery, NewFact, NewFile, Page, RecallQuery, Remembered,
    SpaceInfo, Store, StoreError,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;

const FILE_DOWNLOAD_LIMIT: u64 = 8_000_000;

pub(crate) struct Scope<'a> {
    pub principal: &'a Principal,
    pub store: &'a Store,
}

impl Scope<'_> {
    fn writable(&self, space: &str) -> Result<()> {
        require_write(self.principal.space_permissions(space)?)
    }

    fn check_parent(&self, parent: Option<&str>) -> Result<()> {
        match parent {
            Some(parent) => self.writable(parent),
            None => require_write(self.principal.can_write_root()),
        }
    }

    fn read_spaces(&self, space: Option<&str>) -> Result<Vec<String>> {
        if let Some(space) = space {
            self.principal.space_permissions(space)?;
            return Ok(vec![space.into()]);
        }
        Ok(self.spaces()?.into_iter().map(|space| space.name).collect())
    }

    pub fn spaces(&self) -> Result<Vec<SpaceInfo>> {
        let mut visible = Vec::new();
        for space in self.store.spaces()? {
            match self.principal.space_permissions(&space.name) {
                Ok(_) => visible.push(space),
                Err(StoreError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let names: HashSet<_> = visible.iter().map(|space| space.name.clone()).collect();
        for space in &mut visible {
            if space
                .parent
                .as_ref()
                .is_some_and(|name| !names.contains(name))
            {
                space.parent = None;
            }
        }
        Ok(visible)
    }

    pub fn spaces_view(&self) -> Result<Vec<Value>> {
        self.spaces()?
            .into_iter()
            .map(|space| {
                let writable = self.principal.can_write(&space.name);
                let mut value = output(space)?;
                value["writable"] = json!(writable);
                Ok(value)
            })
            .collect()
    }

    pub fn create_space(&self, name: &str, label: &str, parent: Option<&str>) -> Result<()> {
        // A name already used outside the grant is a hidden resource.
        match self.store.space_chain(name) {
            Ok(_) => {
                self.principal.space_permissions(name)?;
            }
            Err(StoreError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
        self.check_parent(parent)?;
        Ok(self.store.space_create(name, label, parent)?)
    }

    pub fn move_space(
        &self,
        name: &str,
        label: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> Result<()> {
        let writable = self.principal.space_permissions(name)?;
        if let Some(parent) = parent {
            self.check_parent(parent)?;
        }
        require_write(writable)?;
        Ok(self.store.space_move(name, label, parent)?)
    }

    pub fn fact(&self, id: &str) -> Result<Fact> {
        let fact = self.store.read(id)?;
        self.principal.space_permissions(&fact.space)?;
        Ok(fact)
    }

    pub fn write_fact(&self, id: &str) -> Result<Fact> {
        let fact = self.store.read(id)?;
        self.writable(&fact.space)?;
        Ok(fact)
    }

    pub fn visible_fact(&self, mut fact: Fact) -> Result<Fact> {
        if let Some(parent) = &fact.replaces {
            match self.fact(parent) {
                Ok(_) => {}
                Err(ToolError::Store(StoreError::NotFound)) => fact.replaces = None,
                Err(error) => return Err(error),
            }
        }
        Ok(fact)
    }

    pub fn history(&self, id: &str) -> Result<Vec<Fact>> {
        self.fact(id)?;
        let mut history = Vec::new();
        for fact in self.store.history(id)? {
            match self.principal.space_permissions(&fact.space) {
                Ok(_) => history.push(self.visible_fact(fact)?),
                Err(StoreError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(history)
    }

    fn new_fact(space: &str, note: Note, author: String) -> NewFact {
        NewFact {
            space: space.into(),
            text: note.text,
            tags: note.tags,
            author,
            source: "mcp".into(),
            at: note.at,
            origin: note.origin,
        }
    }

    pub fn remember(&self, space: &str, notes: Vec<Note>) -> Result<Vec<Remembered>> {
        self.writable(space)?;
        let notes = notes
            .into_iter()
            .map(|mut note| {
                let author = note
                    .author
                    .take()
                    .filter(|_| self.principal.can_write_root())
                    .unwrap_or_else(|| self.principal.connection_name().to_owned());
                Self::new_fact(space, note, author)
            })
            .collect();
        let results = self.store.remember_many_checked(notes, |result| {
            self.principal
                .space_permissions(&result.fact.space)
                .map(|_| ())
        })?;
        results
            .into_iter()
            .map(|mut result| {
                result.fact = self.visible_fact(result.fact)?;
                Ok(result)
            })
            .collect()
    }

    pub fn supersede(&self, input: Supersede) -> Result<Fact> {
        let previous = self.write_fact(&input.id)?;
        let author = input
            .author
            .filter(|_| self.principal.can_write_root())
            .unwrap_or_else(|| self.principal.connection_name().to_owned());
        let replacement = Self::new_fact(
            &previous.space,
            Note {
                text: input.text,
                tags: input.tags.unwrap_or(previous.tags),
                at: input.at,
                origin: input.origin,
                author: None,
            },
            author,
        );
        Ok(self
            .store
            .supersede(&input.id, replacement, &input.reason)?)
    }

    pub fn move_facts(&self, ids: &[String], target: &str) -> Result<Vec<Fact>> {
        let mut writable = self.principal.space_permissions(target)?;
        for id in ids {
            writable &= self
                .principal
                .space_permissions(&self.store.read(id)?.space)?;
        }
        require_write(writable)?;
        Ok(self.store.move_facts(ids, target)?)
    }

    pub fn recall(&self, query: Recall) -> Result<Vec<Hit>> {
        let spaces = self.read_spaces(query.space.as_deref())?;
        if spaces.is_empty() {
            return Ok(Vec::new());
        }
        let defaults = RecallQuery::default();
        let hits = self.store.recall_with_pin_filter(
            RecallQuery {
                text: query.query,
                spaces,
                tags_any: query.tags,
                since: query.since,
                limit: query.limit.unwrap_or(defaults.limit),
                ..defaults
            },
            query.pinned,
        )?;
        hits.into_iter()
            .map(|mut hit| {
                hit.fact = self.visible_fact(hit.fact)?;
                Ok(hit)
            })
            .collect()
    }

    pub fn list(&self, query: List) -> Result<Value> {
        let spaces = self.read_spaces(query.space.as_deref())?;
        if spaces.is_empty() {
            return output(Page::<Fact> {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let defaults = ListQuery::default();
        let mut page = self.store.list(ListQuery {
            spaces,
            status: match query.status.as_deref() {
                Some("retired") => Some(FactStatus::Retired),
                Some("superseded") => Some(FactStatus::Superseded),
                _ => None,
            },
            pinned: query.pinned,
            tags_any: query.tags,
            since: query.since,
            limit: query.limit.unwrap_or(defaults.limit),
            cursor: query.cursor,
            ..defaults
        })?;
        page.items = page
            .items
            .into_iter()
            .map(|fact| self.visible_fact(fact))
            .collect::<Result<_>>()?;
        let mut result = output(&page)?;
        if query.origin_aliases == Some(true) {
            let ids = page
                .items
                .iter()
                .map(|fact| fact.id.clone())
                .collect::<Vec<_>>();
            result["origin_aliases"] = output(self.store.origin_aliases(&ids)?)?;
        }
        Ok(result)
    }

    pub fn put_file(&self, input: FilePut) -> Result<FileInfo> {
        self.writable(&input.space)?;
        let bytes = STANDARD
            .decode(input.base64)
            .map_err(|_| ToolError::Message(texts::INVALID_BASE64.into()))?;
        let file = NewFile {
            space: input.space,
            name: input.name,
            mime: input
                .mime
                .unwrap_or_else(|| "application/octet-stream".into()),
            reader: bytes.as_slice(),
            max_size: bytes.len() as u64,
            note: input.note,
        };
        if input.if_absent == Some(true) {
            Ok(self.store.file_put_if_absent(file)?)
        } else {
            Ok(self.store.file_put(file)?)
        }
    }

    pub fn get_file(&self, id: &str) -> Result<Value> {
        let (file, path) = self.store.file_get(id)?;
        self.principal.space_permissions(&file.space)?;
        if file.size > FILE_DOWNLOAD_LIMIT {
            return Err(file_too_large(file.size));
        }
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| file.take(FILE_DOWNLOAD_LIMIT + 1).read_to_end(&mut bytes))
            .map_err(StoreError::from)?;
        if bytes.len() as u64 != file.size {
            return Err(ToolError::Internal);
        }
        let mut result = output(file)?;
        result["base64"] = Value::String(STANDARD.encode(bytes));
        Ok(result)
    }

    pub fn list_files(&self, query: FileList) -> Result<Page<FileInfo>> {
        let spaces = self.read_spaces(query.space.as_deref())?;
        if spaces.is_empty() {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let defaults = ListQuery::default();
        Ok(self.store.file_list(ListQuery {
            spaces,
            cursor: query.cursor,
            limit: query.limit.unwrap_or(defaults.limit),
            ..defaults
        })?)
    }

    pub fn retire_file(&self, id: &str, reason: &str) -> Result<()> {
        let (file, _) = self.store.file_get(id)?;
        self.writable(&file.space)?;
        Ok(self.store.file_retire(id, reason)?)
    }
}

fn file_too_large(size: u64) -> ToolError {
    ToolError::Message(texts::download_limit(size, FILE_DOWNLOAD_LIMIT))
}

fn require_write(allowed: bool) -> Result<()> {
    if allowed {
        Ok(())
    } else {
        Err(ToolError::Message(texts::WRITE_REQUIRED.into()))
    }
}
