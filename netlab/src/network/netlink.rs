use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use futures_util::stream::TryStreamExt;
use rtnetlink::packet_route::link::LinkAttribute;

static VETH_ID: AtomicU64 = AtomicU64::new(0);
static LAN_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) async fn link_exists(handle: &rtnetlink::Handle, name: &str) -> Result<bool> {
    let mut links = handle.link().get().execute();

    while let Some(link) = links.try_next().await? {
        if link
            .attributes
            .iter()
            .any(|attr| matches!(attr, LinkAttribute::IfName(if_name) if if_name == name))
        {
            return Ok(true);
        }
    }

    Ok(false)
}

pub(crate) async fn link_index(handle: &rtnetlink::Handle, name: &str) -> Result<u32> {
    let link = handle
        .link()
        .get()
        .match_name(name.to_string())
        .execute()
        .try_next()
        .await?
        .context(format!("failed to find link: {}", name))?;

    Ok(link.header.index)
}

pub(crate) async fn allocate_lan_name(label: &str, handle: &rtnetlink::Handle) -> Result<String> {
    let label: String = label
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(5)
        .collect();
    let label = if label.is_empty() {
        "lan".to_string()
    } else {
        label
    };

    loop {
        let id = LAN_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!("br-{}-{}", label, id);

        if !link_exists(handle, &name).await? {
            return Ok(name);
        }
    }
}

pub(crate) async fn allocate_veth_names(handle: &rtnetlink::Handle) -> Result<(String, String)> {
    loop {
        let id = VETH_ID.fetch_add(1, Ordering::Relaxed);
        let name1 = format!("veth{}a", id);
        let name2 = format!("veth{}b", id);

        if !link_exists(handle, &name1).await? && !link_exists(handle, &name2).await? {
            return Ok((name1, name2));
        }
    }
}
