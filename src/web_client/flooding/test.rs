#[cfg(test)]
mod flood_test{
    use std::{collections::HashMap, fmt::Debug, hash::BuildHasher};
    use itertools::Itertools;
    use petgraph::prelude::{DiGraphMap, GraphMap};
    use wg_2024::{network::SourceRoutingHeader, packet::{FloodResponse, NodeType, Packet}};

    use crate::web_client::{utils_for_test::{client_with_graph_and_nodes_type, COMPLEX_TOPOLOGY}, GraphNodeType, DEFAULT_WEIGHT};

    /// compares two graphmaps
fn graphmap_eq<N: Debug, E: Debug, Ty, Ix>(
    a: &GraphMap<N, E, Ty, Ix>,
    b: &GraphMap<N, E, Ty, Ix>,
) -> bool
where
    N: PartialEq + PartialOrd + std::hash::Hash + Ord + Copy,
    E: PartialEq + Copy + PartialOrd,
    Ty: petgraph::EdgeType,
    Ix: BuildHasher,
{
    // let a_ns = a.nodes();
    // let b_ns = b.nodes();

    let a_es = a.all_edges().map(|e| (e.0, e.1, *e.2));
    let b_es = b.all_edges().map(|e| ((e.0, e.1, *e.2)));
    a_es.sorted_by(|a, b| a.partial_cmp(b).unwrap())
        .eq(b_es.sorted_by(|a, b| a.partial_cmp(b).unwrap()))

    // for (a, b, c) in a_es.sorted_by(|a, b| a.partial_cmp(b).unwrap()) {
    //     print!("{a:?}, {b:?}, {c:?} - ");
    // }
    // println!("\n---");
    // for (a, b, c) in b_es.sorted_by(|a, b| a.partial_cmp(b).unwrap()) {
    //     print!("{a:?}, {b:?}, {c:?} - ");
    // }
    // println!("\n-----");
    // true
}

    #[test]
    pub fn start_flooding_simple_top() {
        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, _s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, _c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::new(),
            HashMap::from([(1, GraphNodeType::Client)]),
        );

        client.start_flooding();
        assert_eq!(client.sequential_flood_id, 1);

        client.handle_packet(Packet::new_flood_response(
            SourceRoutingHeader::new(vec![12, 11, 1], 2),
            0,
            FloodResponse {
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (12, NodeType::Drone),
                ],
                flood_id: 1,
            },
        ));
        client.handle_packet(Packet::new_flood_response(
            SourceRoutingHeader::new(vec![12, 11, 1], 2),
            0,
            FloodResponse {
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (13, NodeType::Drone),
                ],
                flood_id: 1,
            },
        ));
        client.handle_packet(Packet::new_flood_response(
            SourceRoutingHeader::new(vec![12, 11, 1], 2),
            0,
            FloodResponse {
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (14, NodeType::Drone),
                    (13, NodeType::Drone),
                ],
                flood_id: 1,
            },
        ));

        assert!(graphmap_eq(
            &client.topology_graph,
            &GraphMap::from_edges([
                (1, 11, DEFAULT_WEIGHT),
                (11, 1, DEFAULT_WEIGHT),
                (11, 12, DEFAULT_WEIGHT),
                (12, 11, DEFAULT_WEIGHT),
                (13, 11, DEFAULT_WEIGHT),
                (11, 13, DEFAULT_WEIGHT),
                (14, 11, DEFAULT_WEIGHT),
                (11, 14, DEFAULT_WEIGHT),
                (13, 14, DEFAULT_WEIGHT),
                (14, 13, DEFAULT_WEIGHT)
            ])
        ));

        assert_eq!(
            client.nodes_type,
            HashMap::from([
                (1, GraphNodeType::Client),
                (11, GraphNodeType::Drone),
                (12, GraphNodeType::Drone),
                (13, GraphNodeType::Drone),
                (14, GraphNodeType::Drone)
            ])
        )
    }

    #[test]
    pub fn start_flooding_complex_top() {
        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, _s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, _c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::new(),
            HashMap::from([(1, GraphNodeType::Client)]),
        );

        client.start_flooding();
        assert_eq!(client.sequential_flood_id, 1);

        client.handle_packet(Packet::new_flood_response(
            SourceRoutingHeader::new(vec![12, 11, 1], 2),
            0,
            FloodResponse {
                path_trace: vec![
                    (1, NodeType::Client),
                    (12, NodeType::Drone),
                    (11, NodeType::Drone),
                ],
                flood_id: 1,
            },
        ));

        client.handle_packet(Packet::new_flood_response(
            SourceRoutingHeader::new(vec![12, 11, 1], 2),
            0,
            FloodResponse {
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (13, NodeType::Drone),
                    (2, NodeType::Client),
                    (14, NodeType::Drone),
                ],
                flood_id: 1,
            },
        ));

        client.handle_packet(Packet::new_flood_response(
            SourceRoutingHeader::new(vec![12, 11, 1], 2),
            0,
            FloodResponse {
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (14, NodeType::Drone),
                    (13, NodeType::Drone),
                ],
                flood_id: 1,
            },
        ));

        assert!(graphmap_eq(
            &client.topology_graph,
            &GraphMap::from_edges(COMPLEX_TOPOLOGY)
        ));

        assert_eq!(
            client.nodes_type,
            HashMap::from([
                (1, GraphNodeType::Client),
                (11, GraphNodeType::Drone),
                (12, GraphNodeType::Drone),
                (13, GraphNodeType::Drone),
                (14, GraphNodeType::Drone),
                (2, GraphNodeType::Client)
            ])
        )
    }
}