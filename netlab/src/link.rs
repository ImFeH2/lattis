use std::sync::Arc;

use anyhow::Result;
use rtnetlink::{
    LinkMessageBuilder, LinkUnspec, LinkVeth,
    packet_route::link::{InfoData, InfoKind, InfoVeth},
};

use crate::{interface::Interface, netlink::allocate_veth_names, netns::NamespaceNode};

pub(crate) async fn create_veth_pair(
    left_node: Arc<NamespaceNode>,
    right_node: Arc<NamespaceNode>,
) -> Result<(Interface, Interface)> {
    let right_namespace = right_node.namespace.raw_fd();

    let (left_name, right_name) = left_node
        .run_netlink(move |handle| async move {
            let (left_name, right_name) = allocate_veth_names(&handle).await?;

            let peer = LinkMessageBuilder::<LinkUnspec>::new()
                .name(right_name.clone())
                .setns_by_fd(right_namespace)
                .build();
            let veth = LinkMessageBuilder::<LinkVeth>::new_with_info_kind(InfoKind::Veth)
                .name(left_name.clone())
                .set_info_data(InfoData::Veth(InfoVeth::Peer(peer)))
                .build();

            handle.link().add(veth).execute().await?;

            Ok((left_name, right_name))
        })
        .await?;

    let left_interface = Interface::new(left_name, left_node).await?;
    let right_interface = Interface::new(right_name, right_node).await?;

    Ok((left_interface, right_interface))
}
