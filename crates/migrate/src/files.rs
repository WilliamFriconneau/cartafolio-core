use crate::{
    archive::Archive,
    report::Report,
    rpc::{array, next, parse, Client, Reply, Result},
    spaces::Space,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Deserialize)]
struct FileInfo {
    id: String,
    name: String,
    mime: String,
    size: u64,
    note: Option<String>,
}

type Digests = HashSet<(String, String)>;

pub fn copy(
    old: &Client<'_>,
    new: Option<&Client<'_>>,
    space: &Space,
    mut archive: Option<&mut Archive>,
    report: &mut Report,
    limit: u64,
) -> Result<()> {
    let inventory = match inventory(old, &space.old) {
        Ok(inventory) => inventory,
        Err(error) => {
            report.error(error);
            return Ok(());
        }
    };
    let mut held = match new.map(|client| read_back(client, &space.new)).transpose() {
        Ok(held) => held,
        Err(error) => {
            report.error(error);
            None
        }
    };
    for raw in inventory {
        let file: FileInfo = match parse(&raw) {
            Ok(file) => file,
            Err(error) => {
                report.error(error);
                continue;
            }
        };
        let too_large = file.size > limit;
        if too_large {
            report.notice(format!(
                "File {:?} left on source: over {limit} bytes",
                file.id
            ));
        }
        let transfer = !too_large && new.is_some() && held.is_some();
        if !transfer && archive.is_none() {
            continue;
        }
        let bytes = old
            .call("file_get", json!({"id":file.id}))
            .and_then(|reply| bytes(reply, &file));
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                report.error(format!("Source file {:?}: {error}", file.id));
                if let Some(archive) = &mut archive {
                    archive.entry("file", &raw)?;
                }
                continue;
            }
        };
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if let Some(archive) = &mut archive {
            archive.file(&raw, &digest, &bytes)?;
        }
        if let (false, Some(client), Some(held)) = (too_large, new, &mut held) {
            let identity = (file.name.clone(), digest);
            if held.contains(&identity) {
                continue;
            }
            let mut args = json!({"space":space.new,"name":file.name,"mime":file.mime,
                "base64":STANDARD.encode(&bytes),"if_absent":true});
            if let Some(note) = file.note {
                args["note"] = json!(note);
            }
            match client.call("file_put", args) {
                Ok(reply)
                    if reply.data["sha256"] == identity.1 && reply.data["name"] == identity.0 =>
                {
                    held.insert(identity);
                    report.files_sent += 1;
                }
                Ok(_) => report.error(format!(
                    "Destination file {:?}: digest or name differs",
                    file.id
                )),
                Err(error) => report.error(format!("Destination file {:?}: {error}", file.id)),
            }
        }
    }
    Ok(())
}

fn inventory(client: &Client<'_>, space: &str) -> Result<Vec<Value>> {
    let reply = client.call("file_list", json!({"space":space,"limit":1000}))?;
    let files = array(&reply.data["files"])?;
    if files.len() < 1000 {
        return Ok(files.clone());
    }
    // The old file_list stops at 1000 files. Its files view has a cursor.
    let mut args = json!({"space":space,"view":"files","limit":200});
    let mut cursors = HashSet::new();
    let mut inventory = Vec::new();
    loop {
        let page = client.call("space_view", args.clone())?.data;
        inventory.extend(array(&page["items"])?.iter().cloned());
        let Some(cursor) = next(&page["page"]["next_cursor"], &mut cursors)? else {
            return Ok(inventory);
        };
        args["cursor"] = json!(cursor);
    }
}

fn bytes(reply: Reply, file: &FileInfo) -> Result<Vec<u8>> {
    if reply.data["file"]["id"] != file.id {
        return Err("Unexpected source file ID".into());
    }
    let nested = reply.data["mcp_content"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    for block in reply.blocks.iter().chain(nested) {
        let data = block["resource"]["blob"]
            .as_str()
            .or_else(|| block["data"].as_str());
        let bytes = match data {
            Some(data) => STANDARD.decode(data).map_err(|_| "Invalid file base64")?,
            None => match block["text"].as_str() {
                Some(text) => text.as_bytes().to_vec(),
                None => continue,
            },
        };
        if bytes.len() as u64 != file.size {
            return Err("Source file byte length differs from its metadata".into());
        }
        return Ok(bytes);
    }
    Err("Missing source file contents".into())
}

fn read_back(client: &Client<'_>, space: &str) -> Result<Digests> {
    let mut args = json!({"space":space,"limit":200});
    let mut cursors = HashSet::new();
    let mut files = Digests::new();
    loop {
        let page = client.call("file_list", args.clone())?.data;
        for file in array(&page["items"])? {
            files.insert((
                crate::rpc::text(&file["name"])?.into(),
                crate::rpc::text(&file["sha256"])?.into(),
            ));
        }
        let Some(cursor) = next(&page["next_cursor"], &mut cursors)? else {
            return Ok(files);
        };
        args["cursor"] = json!(cursor);
    }
}
