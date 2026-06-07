mod common;

use anyhow::{Context, Result};
use futures_util::stream::TryStreamExt;
use rtnetlink::{LinkUnspec, LinkVeth};

use common::netns::NetworkNamespaceGuard;

async fn find_link_index(handle: &rtnetlink::Handle, name: &str) -> Result<u32> {
    let mut links = handle.link().get().match_name(name.to_string()).execute();
    let link = links
        .try_next()
        .await?
        .context(format!("failed to find link: {}", name))?;
    Ok(link.header.index)
}

#[cfg(target_os = "linux")]
#[ignore = "requires Linux network namespaces"]
#[tokio::test(flavor = "current_thread")]
async fn two_device_direct_link() -> Result<()> {
    let veth1_name = "veth1".to_string();
    let veth2_name = "veth2".to_string();

    let ns1 = NetworkNamespaceGuard::new("dev1").await?;
    let ns2 = NetworkNamespaceGuard::new("dev2").await?;

    let (connection, handle, _) = rtnetlink::new_connection()?;
    tokio::spawn(connection);

    handle
        .link()
        .add(LinkVeth::new(&veth1_name, &veth2_name).build())
        .execute()
        .await?;

    let index1 = find_link_index(&handle, &veth1_name).await?;
    let index2 = find_link_index(&handle, &veth2_name).await?;

    handle
        .link()
        .set(
            LinkUnspec::new_with_index(index1)
                .setns_by_fd(ns1.as_raw_fd())
                .build(),
        )
        .execute()
        .await?;

    handle
        .link()
        .set(
            LinkUnspec::new_with_index(index2)
                .setns_by_fd(ns2.as_raw_fd())
                .build(),
        )
        .execute()
        .await?;

    ns1.with_handle(|handle| async move {
        let index = find_link_index(&handle, &veth1_name).await?;

        handle
            .link()
            .set(LinkUnspec::new_with_index(index).up().build())
            .execute()
            .await?;

        handle
            .address()
            .add(index, std::net::IpAddr::V4("10.10.0.1".parse()?), 24)
            .execute()
            .await?;

        Ok(())
    })
    .await?;

    ns2.with_handle(|handle| async move {
        let index = find_link_index(&handle, &veth2_name).await?;

        handle
            .link()
            .set(LinkUnspec::new_with_index(index).up().build())
            .execute()
            .await?;

        handle
            .address()
            .add(index, std::net::IpAddr::V4("10.10.0.2".parse()?), 24)
            .execute()
            .await?;

        Ok(())
    })
    .await?;

    ns1.run(|| {
        lattis_core::print_info()?;
        Ok(())
    })?;

    ns2.run(|| {
        lattis_core::print_info()?;
        Ok(())
    })?;

    Ok(())
}
