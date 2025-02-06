use std::collections::HashMap;

use common::{
    slc_commands::{WebClientCommand, WebClientEvent},
    Client,
};
use crossbeam_channel::{unbounded, Receiver, Sender};
use petgraph::{prelude::GraphMap, Directed};
use wg_2024::{network::NodeId, packet::Packet};

use super::{GraphNodeType, WebBrowser, DEFAULT_WEIGHT};

pub(crate) const COMPLEX_TOPOLOGY: [(u8, u8, f64); 14] = [
    (1, 11, DEFAULT_WEIGHT),
    (11, 1, DEFAULT_WEIGHT),
    (11, 12, DEFAULT_WEIGHT),
    (12, 11, DEFAULT_WEIGHT),
    (13, 11, DEFAULT_WEIGHT),
    (11, 13, DEFAULT_WEIGHT),
    (14, 11, DEFAULT_WEIGHT),
    (11, 14, DEFAULT_WEIGHT),
    (13, 14, DEFAULT_WEIGHT),
    (14, 13, DEFAULT_WEIGHT),
    (1, 12, DEFAULT_WEIGHT),
    (12, 1, DEFAULT_WEIGHT),
    (13, 2, DEFAULT_WEIGHT),
    (14, 2, DEFAULT_WEIGHT),
];

// useful shotcut to create a client with initialized graph and nodes_type inside tests
pub(crate) fn client_with_graph_and_nodes_type(
    graph: GraphMap<NodeId, f64, Directed>,
    nodes_type: HashMap<NodeId, GraphNodeType>,
) -> (
    WebBrowser,
    (Sender<Packet>, Receiver<Packet>),
    (Sender<Packet>, Receiver<Packet>),
    (Sender<WebClientCommand>, Receiver<WebClientCommand>),
    (Sender<WebClientEvent>, Receiver<WebClientEvent>),
) {
    // Client 1 channels
    let (c_send, c_recv) = unbounded();

    let (c_event_send, c_event_recv) = unbounded();
    let (c_command_send, c_command_recv) = unbounded();
    // neighbor
    let (s_send, s_recv) = unbounded();

    // client 1
    let neighbours1 = HashMap::from([(11, s_send.clone())]);
    let mut client = WebBrowser::new(
        1,
        c_event_send.clone(),
        c_command_recv.clone(),
        c_recv.clone(),
        neighbours1,
    );

    client.topology_graph = graph;
    client.nodes_type = nodes_type;

    (
        client,
        (c_send, c_recv),
        (s_send, s_recv),
        (c_command_send, c_command_recv),
        (c_event_send, c_event_recv),
    )
}
