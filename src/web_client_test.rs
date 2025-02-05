use std::collections::HashMap;

use common::{
    slc_commands::{WebClientCommand, WebClientEvent},
    web_messages::{ResponseMessage, Serializable},
    Client,
};
use compression::{lzw::LZWCompressor, Compressor};
use crossbeam_channel::{unbounded, Receiver, Sender};
use petgraph::{prelude::GraphMap, Directed};
use wg_2024::{
    network::NodeId,
    packet::{Fragment, Packet, FRAGMENT_DSIZE},
};

use crate::{GraphNodeType, WebBrowser};

fn simulate_server_compression(before: ResponseMessage) -> Vec<Fragment> {
    let bytes = LZWCompressor::new()
        .compress(before.serialize().unwrap())
        .unwrap()
        .serialize()
        .unwrap();

    let chunks: std::slice::Chunks<'_, u8> = bytes.chunks(FRAGMENT_DSIZE);
    let n_frag = chunks.len();

    let mut ret = vec![];

    for c in chunks {
        let data = if let Ok(arr) = <[u8; FRAGMENT_DSIZE]>::try_from(c) {
            arr
        } else {
            let mut ret: [u8; FRAGMENT_DSIZE] = [0; FRAGMENT_DSIZE];

            ret[..c.len()].copy_from_slice(c);

            ret
        };

        ret.push(Fragment {
            fragment_index: ret.len() as u64,
            total_n_fragments: n_frag as u64,
            length: c.len() as u8,
            data,
        });
    }
    ret
}

fn client_with_graph_and_nodes_type(
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

#[cfg(test)]
mod web_client_tests {
    use itertools::Itertools;
    use std::fmt::Debug;
    use std::{hash::BuildHasher, vec};

    use common::{
        slc_commands::{WebClientCommand, WebClientEvent},
        web_messages::TextRequest,
    };
    use crossbeam_channel::{unbounded, TryRecvError};
    use petgraph::prelude::GraphMap;

    use crate::web_client_test::client_with_graph_and_nodes_type;
    use crate::{web_client_test::simulate_server_compression, *};

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

    const COMPLEX_TOPOLOGY: [(u8, u8, f64); 14] = [
        (1, 11, DEFAULT_PDR),
        (11, 1, DEFAULT_PDR),
        (11, 12, DEFAULT_PDR),
        (12, 11, DEFAULT_PDR),
        (13, 11, DEFAULT_PDR),
        (11, 13, DEFAULT_PDR),
        (14, 11, DEFAULT_PDR),
        (11, 14, DEFAULT_PDR),
        (13, 14, DEFAULT_PDR),
        (14, 13, DEFAULT_PDR),
        (1, 12, DEFAULT_PDR),
        (12, 1, DEFAULT_PDR),
        (13, 2, DEFAULT_PDR),
        (14, 2, DEFAULT_PDR),
    ];

    #[test]
    pub fn handle_ack() {
        unimplemented!()
    }

    #[test]
    pub fn create_request() {
        let (
            mut client,
            (_, _),
            (_, _),
            (_, _),
            (_, _),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::new(),
            HashMap::from([
                (1, GraphNodeType::Client),
                (22, GraphNodeType::Text),
                (2, GraphNodeType::Media),
                (3, GraphNodeType::Server),
            ]),
        );

        client.create_request(RequestType::TextList(34));
        assert!(client.pending_requests.is_empty());

        client.create_request(RequestType::TextList(22));
        assert_eq!(
            client.pending_requests,
            vec![WebBrowserRequest::new(
                0,
                22,
                HashMap::from([(
                    PacketId::new(),
                    web_messages::RequestMessage::new_text_list_request(client.id, Compression::None)
                        .fragment().unwrap().get(0).unwrap().clone()
                )]),
                Compression::None,
                RequestType::TextList(22)
            )]
        );
        assert_eq!(client.packet_id_counter, PacketId::from_u64(1));
        client.pending_requests = vec![];

        client.create_request(RequestType::ServersType);
        assert!(client.pending_requests.contains(
            &WebBrowserRequest::new(
                1,
                3,
                HashMap::from([(
                    PacketId::from_u64(1),
                    web_messages::RequestMessage::new_type_request(client.id, Compression::None).fragment().unwrap().get(0).unwrap().clone()
                )]),
                Compression::None,
                RequestType::ServersType
            ))
        );


    }

    #[test]
    pub fn complete_request_with_media_response() {
        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::new(),
            HashMap::from([
                (1, GraphNodeType::Client),
                (22, GraphNodeType::Text),
                (2, GraphNodeType::Media),
            ]),
        );

        client.complete_request_with_media_response(
            2,
            RequestType::Media("media.jpg".to_string(), 2),
            MediaResponse::Media("content".as_bytes().to_vec()),
        );
        assert_eq!(
            client.stored_files.get(&"media.jpg".to_string()).unwrap(),
            &"content".as_bytes().to_vec()
        );

        client
            .text_media_map
            .insert((22, "tmp".to_string()), vec!["media1.jpg".to_string()]);
        client
            .media_file_either_owner_or_request_left
            .insert("media1.jpg".to_string(), Either::Right(vec![2]));
        client
            .media_file_either_owner_or_request_left
            .insert("media_not_found.jpg".to_string(), Either::Right(vec![2]));
        client.complete_request_with_media_response(
            2,
            RequestType::MediaList(2),
            MediaResponse::MediaList(vec!["media1.jpg".to_string(), "media2".to_string()]),
        );
        assert_eq!(
            client
                .media_file_either_owner_or_request_left
                .get(&"media1.jpg".to_string())
                .unwrap(),
            &Either::Left(Some(2))
        );
        assert_eq!(
            client
                .media_file_either_owner_or_request_left
                .get(&"media_not_found.jpg".to_string())
                .unwrap(),
            &Either::Left(None)
        );
        assert!(!client.pending_requests.is_empty());
    }

    #[test]
    pub fn complete_request_with_text_response() {
        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::new(),
            HashMap::from([
                (1, GraphNodeType::Client),
                (22, GraphNodeType::Text),
                (2, GraphNodeType::Media),
            ]),
        );

        let filename = "file1.html".to_string();

        client.complete_request_with_text_response(
            22,
            RequestType::TextList(22),
            TextResponse::TextList(vec!["a".to_string()]),
        );
        assert_eq!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::ListOfFiles(vec!["a".to_string()], 22)
        );

        client.complete_request_with_text_response(
            22,
            RequestType::Text(filename.clone(), 22),
            TextResponse::Text("content".as_bytes().to_vec()),
        );

        client.complete_request_with_text_response(
            22,
            RequestType::Text(filename.clone(), 22),
            TextResponse::Text("content<img src=\"media.jpg\"/>".as_bytes().to_vec()),
        );
        assert_eq!(
            client.stored_files.get(&filename).unwrap(),
            &"content<img src=\"media.jpg\"/>".as_bytes().to_vec()
        );
        assert_eq!(
            client.text_media_map.get(&(22, filename)).unwrap(),
            &vec!["media.jpg".to_string()]
        );
        assert!(!client.pending_requests.is_empty());
    }

    #[test]
    pub fn complete_request_with_generic_response() {
        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::new(),
            HashMap::from([(1, GraphNodeType::Client), (2, GraphNodeType::Server)]),
        );

        client.complete_request_with_generic_response(
            2,
            RequestType::ServersType,
            &GenericResponse::InvalidRequest,
        );
        assert_eq!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::UnsupportedRequest
        );

        client.complete_request_with_generic_response(
            2,
            RequestType::ServersType,
            &GenericResponse::NotFound,
        );
        assert_eq!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::UnsupportedRequest
        );

        client.complete_request_with_generic_response(
            2,
            RequestType::ServersType,
            &GenericResponse::Type(ServerType::MediaServer),
        );
        assert_eq!(client.nodes_type.get(&2), Some(&GraphNodeType::Media));
        assert_eq!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::ServersTypes(HashMap::from([(2, ServerType::MediaServer)]))
        );
    }

    #[test]
    pub fn add_new_edge() {
        let (mut client, (_, _), (_, _), (_, _), (_, _)) = client_with_graph_and_nodes_type(
            DiGraphMap::from_edges(COMPLEX_TOPOLOGY),
            HashMap::from([(1, GraphNodeType::Client), (12, GraphNodeType::Drone)]),
        );

        client.add_new_edge(1, 2, 9.);
        assert!(client.topology_graph.contains_edge(1, 2));
        client.add_new_edge(1, 11, 23.);
        assert_eq!(client.topology_graph.edge_weight(1, 11), Some(&DEFAULT_PDR));
    }

    #[test]
    pub fn get_request_index() {
        let (mut client, (_, _), (_, _), (_, _), (_, _)) = client_with_graph_and_nodes_type(
            DiGraphMap::from_edges(COMPLEX_TOPOLOGY),
            HashMap::from([(1, GraphNodeType::Client), (12, GraphNodeType::Drone)]),
        );

        let mut id = PacketId::from_u64(12345678);
        client.pending_requests.push(WebBrowserRequest::new(
            id.get_request_id(),
            21,
            HashMap::new(),
            Compression::LZW,
            RequestType::ServersType,
        ));
        id.increment_request_id();
        client.pending_requests.push(WebBrowserRequest::new(
            id.get_request_id(),
            21,
            HashMap::new(),
            Compression::LZW,
            RequestType::ServersType,
        ));

        assert_eq!(
            client.get_request_index(&Packet::new_ack(
                SourceRoutingHeader::empty_route(),
                PacketId::from_u64(12345678).get_session_id(),
                0
            )),
            Some(0)
        );
        assert_eq!(
            client.get_request_index(&Packet::new_ack(
                SourceRoutingHeader::empty_route(),
                id.get_session_id(),
                0
            )),
            Some(1)
        );
        assert_eq!(
            client.get_request_index(&Packet::new_ack(SourceRoutingHeader::empty_route(), 33, 0)),
            None
        );
    }

    #[test]
    pub fn client_is_destination() {
        let (client, (_, _), (_, _), (_, _), (_, _)) = client_with_graph_and_nodes_type(
            DiGraphMap::from_edges(COMPLEX_TOPOLOGY),
            HashMap::from([(1, GraphNodeType::Client), (12, GraphNodeType::Drone)]),
        );

        let p = Packet::new_ack(SourceRoutingHeader::empty_route(), 14, 45);
        assert!(!client.client_is_destination(&p));

        let p = Packet::new_ack(SourceRoutingHeader::new(vec![4, 3, 2, 1], 122), 9, 0);
        assert!(client.client_is_destination(&p));

        let p: Packet = Packet::new_ack(SourceRoutingHeader::new(vec![4, 3], 122), 9, 0);
        assert!(!client.client_is_destination(&p));
    }

    #[test]
    pub fn remove_node() {
        let (mut client, (_, _), (_, _), (_, _), (_, _)) = client_with_graph_and_nodes_type(
            DiGraphMap::from_edges(COMPLEX_TOPOLOGY),
            HashMap::from([(1, GraphNodeType::Client), (12, GraphNodeType::Drone)]),
        );

        client.packets_sent_counter.insert(12, (34., 12.));
        client.remove_node(12);
        assert!(!client.packets_sent_counter.contains_key(&12));
        assert!(!client.topology_graph.contains_node(12));
        assert_eq!(
            client.nodes_type,
            HashMap::from([(1, GraphNodeType::Client)])
        );
    }

    #[test]
    pub fn prepare_packet_routing() {
        let (
            client,
            (_c_send, _c_recv),
            (_s_send, s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, _c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::from_edges(COMPLEX_TOPOLOGY),
            HashMap::from([(1, GraphNodeType::Client), (12, GraphNodeType::Drone)]),
        );

        let (p, h) = client.prepare_packet_routing(
            Packet::new_ack(SourceRoutingHeader::empty_route(), 14, 45),
            14,
        );
        assert_eq!(
            p.routing_header,
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![1, 11, 14]
            }
        );
        let _ = h.unwrap().send(p.clone()).unwrap();
        assert_eq!(s_recv.recv().unwrap(), p);

        let (_, h) = client.prepare_packet_routing(
            Packet::new_ack(SourceRoutingHeader::empty_route(), 14, 45),
            15,
        );
        assert!(h.is_none());
    }

    #[test]
    pub fn is_correct_server_type() {
        let (
            client,
            (_c_send, _c_recv),
            (_s_send, s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::new(),
            HashMap::from([(1, GraphNodeType::Client), (12, GraphNodeType::Drone)]),
        );

        assert!(client.is_correct_server_type(1, &GraphNodeType::Client));
        assert!(!client.is_correct_server_type(12, &GraphNodeType::Text));
        assert!(!client.is_correct_server_type(123, &GraphNodeType::Client));
    }

    #[test]
    pub fn pdr_estimation() {
        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::from_edges([(1, 11, DEFAULT_PDR), (11, 1, DEFAULT_PDR)]),
            HashMap::from([(1, GraphNodeType::Client)]),
        );

        let header = SourceRoutingHeader::new(vec![1, 11, 12], 0);

        client.packets_sent_counter.insert(11, (0., 0.));
        client.update_pdr_after_ack(&header);
        assert_eq!(client.packets_sent_counter.get(&11).unwrap(), &(1., 0.));
        assert_eq!(client.topology_graph.edge_weight(1, 11).unwrap(), &0.);

        client.update_pdr_after_nack(&header, 11);
        assert_eq!(client.packets_sent_counter.get(&11).unwrap(), &(2., 1.));
        assert_eq!(client.topology_graph.edge_weight(1, 11).unwrap(), &0.5);

        client.update_pdr_after_ack(&header);
        client.update_pdr_after_ack(&header);
        assert_eq!(client.packets_sent_counter.get(&11).unwrap(), &(4., 1.));
        assert_eq!(client.topology_graph.edge_weight(1, 11).unwrap(), &0.25);

        client.update_pdr_after_ack(&header);
        client.update_pdr_after_nack(&header, 11);
        client.update_pdr_after_nack(&header, 11);
        client.update_pdr_after_nack(&header, 11);
        assert_eq!(client.packets_sent_counter.get(&11).unwrap(), &(8., 4.));
        assert_eq!(client.topology_graph.edge_weight(1, 11).unwrap(), &0.5);

        for _ in 0..8 {
            client.update_pdr_after_nack(&header, 11);
        }
        assert_eq!(client.packets_sent_counter.get(&11).unwrap(), &(16., 12.));
        assert_eq!(client.topology_graph.edge_weight(1, 11).unwrap(), &0.75);
    }

    #[test]
    pub fn graph_weight_update() {
        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, _s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, _c_event_recv),
        ) = client_with_graph_and_nodes_type(
            GraphMap::from_edges([
                (1, 11, DEFAULT_PDR),
                (11, 1, DEFAULT_PDR),
                (1, 12, DEFAULT_PDR),
                (12, 1, DEFAULT_PDR),
                (12, 21, DEFAULT_PDR),
                (11, 21, DEFAULT_PDR),
            ]),
            HashMap::from([(1, GraphNodeType::Client)]),
        );

        let header = SourceRoutingHeader::new(vec![1, 11, 12], 0);

        client.packets_sent_counter.insert(11, (0., 0.));
        client.packets_sent_counter.insert(12, (0., 0.));

        client.update_pdr_after_ack(&header);
        client.update_pdr_after_nack(&header, 11);
        client.update_pdr_after_ack(&header);
        client.update_pdr_after_ack(&header);
        client.update_pdr_after_ack(&header);
        client.update_pdr_after_nack(&header, 11);
        client.update_pdr_after_nack(&header, 11);
        client.update_pdr_after_nack(&header, 11);
        client.update_pdr_after_ack(&header);
        client.update_pdr_after_ack(&header);
        client.update_pdr_after_ack(&header);
        client.update_pdr_after_nack(&header, 12);
        assert_eq!(
            *client.topology_graph.edge_weight(client.id, 11).unwrap(),
            4. / 11.
        );

        assert_eq!(
            *client.topology_graph.edge_weight(client.id, 12).unwrap(),
            1. / 8.
        );
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
                (1, 11, DEFAULT_PDR),
                (11, 1, DEFAULT_PDR),
                (11, 12, DEFAULT_PDR),
                (12, 11, DEFAULT_PDR),
                (13, 11, DEFAULT_PDR),
                (11, 13, DEFAULT_PDR),
                (14, 11, DEFAULT_PDR),
                (11, 14, DEFAULT_PDR),
                (13, 14, DEFAULT_PDR),
                (14, 13, DEFAULT_PDR)
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

    #[test]
    pub fn add_sender() {
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

        let (d_send, d_recv) = unbounded();

        client.handle_command(WebClientCommand::AddSender(11, d_send.clone()));

        assert_eq!(
            d_recv.recv().unwrap(),
            Packet::new_flood_request(
                SourceRoutingHeader::empty_route(),
                0,
                FloodRequest::initialize(1, 1, NodeType::Client)
            )
        );
    }

    #[test]
    pub fn remove_sender() {
        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, _s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, _c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::from_edges([(1, 11, DEFAULT_PDR), (11, 1, DEFAULT_PDR)]),
            HashMap::from([(1, GraphNodeType::Client), (11, GraphNodeType::Drone)]),
        );

        assert_eq!(client.packets_sent_counter, HashMap::from([(11, (0., 0.))]));

        client.handle_command(WebClientCommand::RemoveSender(11));

        assert_eq!(client.topology_graph.edge_count(), 0);
        assert!(client.topology_graph.contains_node(1));
        assert!(!client.topology_graph.contains_node(11));
        assert_eq!(
            client.nodes_type,
            HashMap::from([(1, GraphNodeType::Client)])
        );
        assert_eq!(client.packets_sent_counter, HashMap::new());
    }

    #[test]
    pub fn file_list_scl_command_1() {
        // client 1 <--> 11 <--> 21 server

        let (
            mut client,
            (_c_send, _c_recv),
            (_s_send, s_recv),
            (_c_command_send, _c_command_recv),
            (_c_event_send, c_event_recv),
        ) = client_with_graph_and_nodes_type(
            DiGraphMap::from_edges([
                (1, 11, DEFAULT_PDR),
                (11, 1, DEFAULT_PDR),
                (11, 21, DEFAULT_PDR),
            ]),
            HashMap::from([
                (1, GraphNodeType::Client),
                (11, GraphNodeType::Drone),
                (21, GraphNodeType::Text),
            ]),
        );

        client.handle_command(WebClientCommand::AskListOfFiles(21));
        // receive request
        let req = s_recv.recv().unwrap();
        //println!("--{:?}", req);
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );
        // remove packet sent
        c_event_recv.recv().unwrap();

        // response
        let data = web_messages::ResponseMessage::new_text_list_response(
            21,
            Compression::None,
            vec!["file1".to_string(), "file2".to_string()],
        )
        .fragment()
        .unwrap();
        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 2,
                hops: vec![21, 11, 1],
            },
            1 << 50,
            data[0].clone(),
        ));

        // send ACK
        client.handle_packet(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 2,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            0,
        ));
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 1 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        client.try_complete_request();

        // remove packet sent
        c_event_recv.recv().unwrap();

        let resp = c_event_recv.recv().unwrap();
        //println!("--{:?}", resp);

        if let WebClientEvent::ListOfFiles(files, id) = resp {
            assert_eq!(files, vec!["file1".to_string(), "file2".to_string()]);
            assert_eq!(id, 21);
        } else {
            assert!(false)
        }
    }

    #[test]
    pub fn command_with_nack() {
        // client 1 <--> 11 <--> 21 server

        let (mut client, (_, _), (_s_send, s_recv), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Text),
                ]),
            );

        client.handle_command(WebClientCommand::AskListOfFiles(21));

        // receive request
        let req = s_recv.recv().unwrap();
        //println!("{:?}", req);
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        // remove packet sent
        c_event_recv.recv().unwrap();

        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );

        // NACK (dropped)
        let nack = Packet::new_nack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            Nack {
                fragment_index: 0,
                nack_type: NackType::Dropped,
            },
        );
        client.handle_packet(nack);

        // receive request second time
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        // remove packet sent
        c_event_recv.recv().unwrap();

        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );

        // response after nack
        let data = web_messages::ResponseMessage::new_text_list_response(
            21,
            Compression::None,
            vec!["file1".to_string(), "file2".to_string()],
        )
        .fragment()
        .unwrap();
        assert_eq!(data.len(), 1);

        // ACK
        client.handle_packet(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            0,
        ));

        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            1 << 50,
            data[0].clone(),
        ));

        client.try_complete_request();

        // remove packet sent
        c_event_recv.recv().unwrap();

        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 1 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        let resp = c_event_recv.recv().unwrap();
        //println!("--{:?}", resp);

        if let WebClientEvent::ListOfFiles(files, id) = resp {
            assert_eq!(files, vec!["file1".to_string(), "file2".to_string()]);
            assert_eq!(id, 21);
        } else {
            assert!(false)
        }
    }

    #[test]
    pub fn command_with_3_nack() {
        // client 1 <--> 11 <--> 21 server

        let (mut client, (_, _), (_s_send, s_recv), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Text),
                ]),
            );

        client.handle_command(WebClientCommand::AskListOfFiles(21));

        // receive request
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        // remove packet sent
        c_event_recv.recv().unwrap();

        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );

        // send 2 "dropped" nacks
        for _ in 0..2 {
            let nack = Packet::new_nack(
                SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![21, 11, 1],
                },
                req.session_id,
                Nack {
                    fragment_index: 0,
                    nack_type: NackType::Dropped,
                },
            );
            client.handle_packet(nack);
            // receive request second time
            let req = s_recv.recv().unwrap();
            //println!("---{:?}", req);

            let mut data = Vec::new();
            match req.pack_type {
                PacketType::MsgFragment(f) => data.push(f),
                _ => {}
            };
            let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
            // remove packet sent
            c_event_recv.recv().unwrap();

            assert_eq!(
                req_defrag,
                RequestMessage {
                    compression_type: Compression::None,
                    source_id: 1,
                    content: web_messages::Request::Text(TextRequest::TextList)
                }
            );
        }

        // NACK 3 (error in routing(11))
        let nack = Packet::new_nack(
            SourceRoutingHeader {
                hop_index: 2,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            Nack {
                fragment_index: 0,
                nack_type: NackType::ErrorInRouting(11),
            },
        );
        client.handle_packet(nack);
        assert_eq!(client.topology_graph.edge_count(), 0);
        assert_eq!(client.topology_graph.node_count(), 2);
        // remove flood request (the node 11 has been removed -> disconnected graph)
        assert!(matches!(
            s_recv.recv().unwrap().pack_type,
            PacketType::FloodRequest(_)
        ));

        client.topology_graph = GraphMap::from_edges([
            (1, 11, DEFAULT_PDR),
            (11, 1, DEFAULT_PDR),
            (11, 21, DEFAULT_PDR),
        ]);
        client.nodes_type = HashMap::from([
            (1, GraphNodeType::Client),
            (11, GraphNodeType::Drone),
            (21, GraphNodeType::Text),
        ]);

        client.try_resend_packet();

        // receive request fourth time
        let req = s_recv.recv().unwrap();
        //println!("--{:?}", req);
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        // remove packet sent
        assert!(matches!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::PacketSent(_)
        ));

        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );

        // response
        let data = web_messages::ResponseMessage::new_text_list_response(
            21,
            Compression::None,
            vec!["file1".to_string(), "file2".to_string()],
        )
        .fragment()
        .unwrap();
        assert_eq!(data.len(), 1);

        // ACK to client (sent later)
        client.handle_packet(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 2,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            0,
        ));
        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 2,
                hops: vec![21, 11, 1],
            },
            1 << 50,
            data[0].clone(),
        ));

        // ack from client
        c_event_recv.recv().unwrap();
        // remove packet sent
        c_event_recv.recv().unwrap();

        client.try_complete_request();

        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 1 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        let resp = c_event_recv.recv().unwrap();
        //println!("--{:?}", resp);

        if let WebClientEvent::ListOfFiles(files, id) = resp {
            assert_eq!(files, vec!["file1".to_string(), "file2".to_string()]);
            assert_eq!(id, 21);
        } else {
            assert!(false)
        }
    }

    #[test]
    pub fn server_type_request() {
        // client 1 <--> 11 <--> 21 server

        let (mut client, (_, _), (_s_send, s_recv), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Server),
                ]),
            );

        client.handle_command(WebClientCommand::AskServersTypes);

        // receive request
        let req = s_recv.recv().unwrap();
        //println!("{:?}", req);
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let resp = RequestMessage::defragment(&data, Compression::None).unwrap();
        c_event_recv.recv().unwrap();

        assert_eq!(
            resp,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Type
            }
        );
        // send ACK to client
        client.handle_packet(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 2,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            0,
        ));

        // response
        let data = web_messages::ResponseMessage::new_type_response(
            21,
            Compression::None,
            ServerType::FileServer,
        )
        .fragment()
        .unwrap();
        assert_eq!(data.len(), 1);

        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 2,
                hops: vec![21, 11, 1],
            },
            3 << 50,
            data[0].clone(),
        ));

        // control ACK from client
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 3 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        // remove packet sent
        c_event_recv.recv().unwrap();

        client.try_complete_request();

        // client response to scl
        let resp = c_event_recv.recv().unwrap();
        //println!("--{:?}", resp);

        if let WebClientEvent::ServersTypes(map) = resp {
            assert_eq!(map, HashMap::from([(21, ServerType::FileServer)]));
        } else {
            assert!(false)
        }
    }

    #[test]
    pub fn file_request_no_media() {
        let (mut client, (_, _), (_, _), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Server),
                ]),
            );

        client.packet_id_counter = PacketId::from_u64(1);

        let html = ResponseMessage::new_text_response(
            21,
            Compression::LZW,
            "<html><h1>ciao</h1></html>".as_bytes().to_vec(),
        );
        let ret = simulate_server_compression(html.clone());
        let req = WebBrowserRequest {
            request_id: 0,
            server_id: 21,
            waiting_for_ack: HashMap::new(),
            incoming_messages: ret,
            compression: Compression::LZW,
            request_type: RequestType::Text("public/file1.html".to_string(), 21),
            response_is_complete: true,
        };
        client.pending_requests.push(req);
        client.try_complete_request();

        // remove packetsetn event from queue
        assert_eq!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::FileFromClient(
                TextMediaResponse::new(
                    (
                        "file1.html".to_string(),
                        "<html><h1>ciao</h1></html>".as_bytes().to_vec()
                    ),
                    vec![]
                ),
                21
            )
        );
    }

    #[test]
    pub fn file_request_one_media() {
        let (mut client, (_, _), (_s_send, s_recv), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Media),
                ]),
            );
        client.packet_id_counter = PacketId::from_u64(1);

        let html = ResponseMessage::new_text_response(
            21,
            Compression::LZW,
            "<html><img src=\"a/b/c/media.jpg\"/>".as_bytes().to_vec(),
        );
        let ret = simulate_server_compression(html.clone());
        let req = WebBrowserRequest {
            request_id: 0,
            server_id: 21,
            waiting_for_ack: HashMap::new(),
            incoming_messages: ret,
            compression: Compression::LZW,
            request_type: RequestType::Text("aa/file1.html".to_string(), 21),
            response_is_complete: true,
        };

        client.pending_requests.push(req);
        client.try_complete_request();

        assert_eq!(
            client.stored_files.get("aa/file1.html").unwrap(),
            &"<html><img src=\"a/b/c/media.jpg\"/>".as_bytes().to_vec()
        );
        assert!(client.stored_files.get("a/b/c/media.jpg").is_none());
        assert_eq!(
            client
                .text_media_map
                .get(&(21, "aa/file1.html".to_string()))
                .unwrap(),
            &vec!["a/b/c/media.jpg".to_string()]
        );
        assert_eq!(
            client.media_file_either_owner_or_request_left,
            HashMap::from([("a/b/c/media.jpg".to_string(), Either::Right(vec![21]))])
        );

        // media file list request
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 11, 21]),
                1,
                RequestMessage::new_media_list_request(1, Compression::None)
                    .fragment()
                    .unwrap()
                    .get(0)
                    .unwrap()
                    .clone()
            )
        );
        // ack
        client.pending_requests.get_mut(0).unwrap().waiting_for_ack = HashMap::new();
        // remove packet sent
        c_event_recv.recv().unwrap();

        // media file list response
        let mut id = PacketId::from_u64(1);
        id.increment_packet_id();
        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader::with_first_hop(vec![21, 1]),
            id.get_session_id(),
            ResponseMessage::new_media_list_response(
                21,
                Compression::None,
                vec!["a.mp3".to_string(), "a/b/c/media.jpg".to_string()],
            )
            .fragment()
            .unwrap()
            .get(0)
            .unwrap()
            .clone(),
        ));

        assert_eq!(
            client.media_file_either_owner_or_request_left,
            HashMap::from([("a/b/c/media.jpg".to_string(), Either::Right(vec![21]))])
        );
        assert!(client
            .stored_files
            .get(&"a/b/c/media.jpg".to_string())
            .is_none());

        client.try_complete_request();

        assert_eq!(
            client.media_file_either_owner_or_request_left,
            HashMap::from([("a/b/c/media.jpg".to_string(), Either::Left(Some(21)))])
        );
        assert!(client
            .stored_files
            .get(&"a/b/c/media.jpg".to_string())
            .is_none());

        assert_eq!(
            client.pending_requests.get(0).unwrap().request_type,
            RequestType::Media("a/b/c/media.jpg".to_string(), 21)
        );
        // ack from client
        s_recv.recv().unwrap();
        // remove packet sent
        c_event_recv.recv().unwrap();

        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 11, 21]),
                2,
                RequestMessage::new_media_request(
                    1,
                    Compression::None,
                    "a/b/c/media.jpg".to_string()
                )
                .fragment()
                .unwrap()
                .get(0)
                .unwrap()
                .clone()
            )
        );
        // ack to client (simulated)
        client.pending_requests.get_mut(0).unwrap().waiting_for_ack = HashMap::new();
        // remove packet sent
        c_event_recv.recv().unwrap();

        // media file response
        let mut id = PacketId::from_u64(2);
        id.increment_packet_id();
        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader::with_first_hop(vec![21, 1]),
            id.get_session_id(),
            ResponseMessage::new_media_response(21, Compression::None, vec![1, 2, 3, 4, 5])
                .fragment()
                .unwrap()
                .get(0)
                .unwrap()
                .clone(),
        ));

        client.try_complete_request();

        // remove packet sent
        c_event_recv.recv().unwrap();

        assert_eq!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::FileFromClient(
                TextMediaResponse::new(
                    (
                        "file1.html".to_string(),
                        "<html><img src=\"a/b/c/media.jpg\"/>".as_bytes().to_vec()
                    ),
                    vec![("media.jpg".to_string(), vec![1, 2, 3, 4, 5])]
                ),
                21
            )
        );

        assert!(client.media_file_either_owner_or_request_left.is_empty());
        assert!(client.pending_requests.is_empty());
        assert!(client.stored_files.is_empty());
    }

    #[test]
    pub fn file_request_three_media_third_unavailable() {
        let (mut client, (_, _), (_s_send, s_recv), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Media),
                ]),
            );
        client.packet_id_counter = PacketId::from_u64(1);

        let html = "<html><img src=\"a/b/c/media.jpg\"/>fieub<img src=\"a/media2.jpg\"/>fieub<img src=\"c/media3.jpg\"/>fieub";
        let html_response =
            ResponseMessage::new_text_response(21, Compression::LZW, html.as_bytes().to_vec());
        let html_frags = simulate_server_compression(html_response.clone());
        let req = WebBrowserRequest {
            request_id: 0,
            server_id: 21,
            waiting_for_ack: HashMap::new(),
            incoming_messages: html_frags,
            compression: Compression::LZW,
            request_type: RequestType::Text("file1.html".to_string(), 21),
            response_is_complete: true,
        };

        client.pending_requests.push(req);
        client.try_complete_request();

        assert_eq!(
            client.stored_files,
            HashMap::from([("file1.html".to_string(), html.as_bytes().to_vec())])
        );
        assert_eq!(
            client
                .text_media_map
                .get(&(21, "file1.html".to_string()))
                .unwrap(),
            &vec![
                "a/b/c/media.jpg".to_string(),
                "a/media2.jpg".to_string(),
                "c/media3.jpg".to_string()
            ]
        );
        assert_eq!(
            client.media_file_either_owner_or_request_left,
            HashMap::from([
                ("a/b/c/media.jpg".to_string(), Either::Right(vec![21])),
                ("a/media2.jpg".to_string(), Either::Right(vec![21])),
                ("c/media3.jpg".to_string(), Either::Right(vec![21]))
            ])
        );

        // media file list request
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 11, 21]),
                1,
                RequestMessage::new_media_list_request(1, Compression::None)
                    .fragment()
                    .unwrap()
                    .get(0)
                    .unwrap()
                    .clone()
            )
        );
        // remove packetsent event from queue
        c_event_recv.recv().unwrap();

        // ack
        client.pending_requests.get_mut(0).unwrap().waiting_for_ack = HashMap::new();

        // media file list response
        let mut id = PacketId::from_u64(1);
        id.increment_packet_id();
        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader::with_first_hop(vec![21, 11, 1]),
            id.get_session_id(),
            ResponseMessage::new_media_list_response(
                21,
                Compression::None,
                vec!["a/media2.jpg".to_string(), "a/b/c/media.jpg".to_string()],
            )
            .fragment()
            .unwrap()
            .get(0)
            .unwrap()
            .clone(),
        ));
        id.increment_request_id();

        client.try_complete_request();

        assert_eq!(
            client.media_file_either_owner_or_request_left,
            HashMap::from([
                ("a/media2.jpg".to_string(), Either::Left(Some(21))),
                ("a/b/c/media.jpg".to_string(), Either::Left(Some(21))),
                ("c/media3.jpg".to_string(), Either::Left(None))
            ])
        );
        assert_eq!(
            client.stored_files,
            HashMap::from([("file1.html".to_string(), html.as_bytes().to_vec())])
        );
        assert!(client
            .pending_requests
            .iter()
            .position(
                |req| req.request_type == RequestType::Media("a/b/c/media.jpg".to_string(), 21)
            )
            .is_some());
        assert!(client
            .pending_requests
            .iter()
            .position(|req| req.request_type == RequestType::Media("a/media2.jpg".to_string(), 21))
            .is_some());

        // media2 request

        // ack
        s_recv.recv().unwrap();
        // remove packetsent event from queue
        c_event_recv.recv().unwrap();

        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 11, 21]),
                2,
                RequestMessage::new_media_request(1, Compression::None, "a/media2.jpg".to_string())
                    .fragment()
                    .unwrap()
                    .get(0)
                    .unwrap()
                    .clone()
            )
        );
        // remove packetsent event from queue
        c_event_recv.recv().unwrap();

        // ack
        client.pending_requests.get_mut(0).unwrap().waiting_for_ack = HashMap::new();
        // remove packetsent event from queue
        c_event_recv.recv().unwrap();

        // media file response
        let mut id = PacketId::from_u64(2);
        id.increment_packet_id();
        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader::with_first_hop(vec![21, 11, 1]),
            id.get_session_id(),
            ResponseMessage::new_media_response(21, Compression::None, vec![2, 4, 6, 8, 10])
                .fragment()
                .unwrap()
                .get(0)
                .unwrap()
                .clone(),
        ));
        client.try_complete_request();
        id.increment_request_id();

        // request media
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 11, 21]),
                id.get_session_id(),
                RequestMessage::new_media_request(
                    1,
                    Compression::None,
                    "a/b/c/media.jpg".to_string()
                )
                .fragment()
                .unwrap()
                .get(0)
                .unwrap()
                .clone()
            )
        );
        // remove packetsent event from queue
        c_event_recv.recv().unwrap();

        // ack
        client.pending_requests.get_mut(0).unwrap().waiting_for_ack = HashMap::new();
        // media file response
        id.increment_packet_id();
        client.handle_packet(Packet::new_fragment(
            SourceRoutingHeader::with_first_hop(vec![21, 1]),
            id.get_session_id(),
            ResponseMessage::new_media_response(21, Compression::None, vec![1, 2, 3, 4, 5])
                .fragment()
                .unwrap()
                .get(0)
                .unwrap()
                .clone(),
        ));

        client.try_complete_request();

        // remove packetsent event from queue
        c_event_recv.recv().unwrap();

        assert_eq!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::FileFromClient(
                TextMediaResponse::new(
                    ("file1.html".to_string(), html.as_bytes().to_vec()),
                    vec![
                        ("media.jpg".to_string(), vec![1, 2, 3, 4, 5]),
                        ("media2.jpg".to_string(), vec![2, 4, 6, 8, 10]),
                        ("media3.jpg".to_string(), vec![])
                    ],
                ),
                21
            )
        );

        assert!(client.media_file_either_owner_or_request_left.is_empty());
        assert!(client.pending_requests.is_empty());
        assert!(client.stored_files.is_empty());
    }

    #[test]
    pub fn try_resend_packet_successfully() {
        let (mut client, (_, _), (_s_send, s_recv), (_, _), (_, _)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Media),
                ]),
            );

        let frag = Fragment::from_string(0, 1, "ciao".to_string());
        let id = PacketId::from_u64(1 << 40);

        let req = WebBrowserRequest {
            request_id: id.get_request_id(),
            server_id: 21,
            waiting_for_ack: HashMap::from([(id.clone(), frag.clone())]),
            incoming_messages: Vec::new(),
            compression: Compression::LZW,
            request_type: RequestType::Text("aa/file1.html".to_string(), 21),
            response_is_complete: true,
        };
        client.pending_requests.push(req);
        client
            .packets_to_bo_sent_again
            .push_back((id.clone(), frag.clone()));

        client.try_resend_packet();

        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 11, 21]),
                id.get_session_id(),
                frag.clone()
            )
        );
        assert!(client.packets_to_bo_sent_again.is_empty());
    }

    #[test]
    pub fn try_resend_packet_unsuccessfully() {
        let (mut client, (_, _), (_, _), (_, _), (_, _)) = client_with_graph_and_nodes_type(
            GraphMap::from_edges([
                (1, 11, DEFAULT_PDR),
                (11, 1, DEFAULT_PDR),
                (11, 21, DEFAULT_PDR),
            ]),
            HashMap::from([
                (1, GraphNodeType::Client),
                (11, GraphNodeType::Drone),
                (21, GraphNodeType::Media),
            ]),
        );

        let frag = Fragment::from_string(0, 1, "ciao".to_string());
        let id = PacketId::from_u64(1 << 40);

        let req = WebBrowserRequest {
            request_id: id.get_request_id(),
            server_id: 22,
            waiting_for_ack: HashMap::from([(id.clone(), frag.clone())]),
            incoming_messages: Vec::new(),
            compression: Compression::LZW,
            request_type: RequestType::Text("aa/file1.html".to_string(), 21),
            response_is_complete: true,
        };
        client.pending_requests.push(req);
        client
            .packets_to_bo_sent_again
            .push_back((id.clone(), frag.clone()));

        client.try_resend_packet();

        assert_eq!(
            client.packets_to_bo_sent_again,
            VecDeque::from([(id, frag)])
        );
    }

    #[test]
    pub fn internal_send_to_controller() {
        let (client, (_, _), (_, _), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Media),
                ]),
            );

        client.internal_send_to_controller(WebClientEvent::UnsupportedRequest);
        assert_eq!(
            c_event_recv.recv().unwrap(),
            WebClientEvent::UnsupportedRequest
        );
    }

    #[test]
    pub fn shortcut_successfully() {
        let (client, (_, _), (_, _), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Media),
                ]),
            );

        let p = Packet::new_ack(SourceRoutingHeader::new(vec![1, 2], 56), 1213, 34);

        client.shortcut(p.clone());
        assert_eq!(c_event_recv.recv().unwrap(), WebClientEvent::Shortcut(p));
    }

    #[test]
    pub fn shortcut_unsuccessfully() {
        let (client, (_, _), (_, _), (_, _), (_c_event_send, c_event_recv)) =
            client_with_graph_and_nodes_type(
                GraphMap::from_edges([
                    (1, 11, DEFAULT_PDR),
                    (11, 1, DEFAULT_PDR),
                    (11, 21, DEFAULT_PDR),
                ]),
                HashMap::from([
                    (1, GraphNodeType::Client),
                    (11, GraphNodeType::Drone),
                    (21, GraphNodeType::Media),
                ]),
            );

        let p = Packet::new_ack(SourceRoutingHeader::empty_route(), 1213, 34);

        client.shortcut(p.clone());
        assert_eq!(c_event_recv.try_recv(), Err(TryRecvError::Empty));
    }
}

#[cfg(test)]
mod fragmentation_tests {
    use common::web_messages::{Compression, RequestMessage, ResponseMessage};

    use crate::{web_client_test::simulate_server_compression, Fragmentable};

    #[test]
    fn invalid_request_response() {
        let before = ResponseMessage::new_invalid_request_response(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn media_list_response() {
        let before = ResponseMessage::new_media_list_response(
            21,
            Compression::None,
            vec!["a, b, c".to_string(), "d".to_string()],
        );
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn media_response() {
        let before = ResponseMessage::new_media_response(21, Compression::None, r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed in lobortis ex. Fusce gravida pharetra lacus, sed ornare quam aliquam sit amet. In hac habitasse platea dictumst. Vestibulum tristique est et varius cursus. Donec tincidunt suscipit augue, eget porta enim porta at. Integer rutrum viverra dictum. Donec neque justo, euismod nec sem vel, porttitor lacinia nunc. Vestibulum sagittis, metus sed tempus tempus, lorem metus placerat velit, sit amet blandit arcu quam imperdiet est. Sed pulvinar erat ut diam bibendum euismod. Proin fermentum nec velit eget fermentum. Phasellus non dapibus urna, eget tempus neque. Aliquam id nibh sed nunc condimentum tempus ac et dui.
Etiam eget sollicitudin massa. Nam et sem sit amet sem facilisis vulputate quis sed ante. In vel mi ut nulla commodo facilisis ac eu orci. Suspendisse potenti. Cras sollicitudin volutpat diam ut porttitor. Nam non tellus eros. Mauris eu interdum augue, quis tincidunt odio."#.as_bytes().to_vec());
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn not_found_response() {
        let before = ResponseMessage::new_not_found_response(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_list_response() {
        let before = ResponseMessage::new_text_list_response(
            21,
            Compression::None,
            vec!["a".to_string(), "b".to_string()],
        );
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_response() {
        let before = ResponseMessage::new_text_response(21, Compression::None, r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed in lobortis ex. Fusce gravida pharetra lacus, sed ornare quam aliquam sit amet. In hac habitasse platea dictumst. Vestibulum tristique est et varius cursus. Donec tincidunt suscipit augue, eget porta enim porta at. Integer rutrum viverra dictum. Donec neque justo, euismod nec sem vel, porttitor lacinia nunc. Vestibulum sagittis, metus sed tempus tempus, lorem metus placerat velit, sit amet blandit arcu quam imperdiet est. Sed pulvinar erat ut diam bibendum euismod. Proin fermentum nec velit eget fermentum. Phasellus non dapibus urna, eget tempus neque. Aliquam id nibh sed nunc condimentum tempus ac et dui.
Etiam eget sollicitudin massa. Nam et sem sit amet sem facilisis vulputate quis sed ante. In vel mi ut nulla commodo facilisis ac eu orci. Suspendisse potenti. Cras sollicitudin volutpat diam ut porttitor. Nam non tellus eros. Mauris eu interdum augue, quis tincidunt odio."#.as_bytes().to_vec());
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn type_response() {
        let before = ResponseMessage::new_type_response(
            21,
            Compression::None,
            common::slc_commands::ServerType::FileServer,
        );
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn media_list_request() {
        let before = RequestMessage::new_media_list_request(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn media_request() {
        let before = RequestMessage::new_media_request(21, Compression::None, r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed in lobortis ex. Fusce gravida pharetra lacus, sed ornare quam aliquam sit amet. In hac habitasse platea dictumst. Vestibulum tristique est et varius cursus. Donec tincidunt suscipit augue, eget porta enim porta at. Integer rutrum viverra dictum. Donec neque justo, euismod nec sem vel, porttitor lacinia nunc. Vestibulum sagittis, metus sed tempus tempus, lorem metus placerat velit, sit amet blandit arcu quam imperdiet est. Sed pulvinar erat ut diam bibendum euismod. Proin fermentum nec velit eget fermentum. Phasellus non dapibus urna, eget tempus neque. Aliquam id nibh sed nunc condimentum tempus ac et dui.
Etiam eget sollicitudin massa. Nam et sem sit amet sem facilisis vulputate quis sed ante. In vel mi ut nulla commodo facilisis ac eu orci. Suspendisse potenti. Cras sollicitudin volutpat diam ut porttitor. Nam non tellus eros. Mauris eu interdum augue, quis tincidunt odio."#.to_string());
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_list_request() {
        let before = RequestMessage::new_text_list_request(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_request() {
        let before = RequestMessage::new_text_request(21, Compression::None, r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Integer congue lobortis metus, gravida dictum sem. Vivamus accumsan felis nunc, eu aliquam lacus ornare non. Curabitur volutpat ante non lacus gravida varius. Curabitur auctor odio a condimentum sodales. Ut euismod volutpat libero. Pellentesque odio dui, lobortis vitae tincidunt ut, euismod id quam. Praesent a orci molestie leo sodales sagittis.

Pellentesque mattis, quam non facilisis interdum, metus eros placerat velit, ac congue turpis turpis quis velit. Nulla metus ex, ultricies sit amet luctus vitae, lacinia sit amet eros. Nam sit amet ullamcorper libero, nec viverra purus. Nulla posuere ante non porttitor euismod. Cras id arcu erat. In faucibus dignissim ipsum. Nullam eu libero sem. In hendrerit porta tellus, sed facilisis libero suscipit vestibulum. Sed mauris dui, elementum pretium lacus eu, porttitor dapibus ex. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Cras et nibh ac lectus cursus sodales sed et diam. Maecenas est ex, volutpat mollis orci ac, imperdiet eleifend tellus. In purus erat, egestas in faucibus quis, lacinia sed erat. Sed consectetur ligula vel nisi convallis, sed dapibus arcu aliquet. Praesent faucibus nulla vehicula, gravida tortor consequat, placerat arcu.
Sed ullamcorper sem elementum enim euismod ornare. In viverra odio at quam maximus porttitor. Pellentesque convallis vitae libero ultricies feugiat. Fusce nec rutrum quam. Sed hendrerit in leo vel tristique. Sed luctus dolor mauris. Cras rhoncus sodales justo, et ultricies urna aliquam vitae. In sollicitudin tincidunt mauris et imperdiet. Donec scelerisque sem id urna semper laoreet. Vivamus pharetra dolor et nunc aliquam lobortis. Vivamus efficitur ligula id libero sollicitudin rhoncus. Nulla mollis urna tincidunt, fermentum dolor et, dignissim purus. Quisque eu varius odio. Nunc cursus ex sit amet orci malesuada volutpat vitae non libero. Sed vitae nisl at risus luctus molestie quis semper metus.
Sed tempus erat id cursus luctus. Pellentesque consectetur lobortis nunc, non iaculis velit dapibus non. In consectetur sapien risus, ullamcorper sagittis leo dignissim et. Nulla eget nulla lorem. Proin enim massa, pretium id lacus ac, hendrerit faucibus nunc. Nam non quam at quam sodales pulvinar in vitae orci. Donec porta iaculis pharetra. Maecenas faucibus diam a mauris feugiat tristique. In sed est dictum, suscipit ante et, ultricies ante. Donec arcu leo, lobortis ac justo ac, bibendum congue lacus. Donec suscipit lacus in mi tempus eleifend. Phasellus sollicitudin aliquet lorem, a bibendum sapien ultrices nec. Nulla eget eros ac urna commodo feugiat a quis nulla. Suspendisse justo turpis, porta non pretium at, porta a arcu.
Pellentesque eget elit vulputate, eleifend arcu eu, maximus risus. Donec vitae scelerisque nisl. Ut dictum volutpat ante, ac maximus felis pulvinar eget. Quisque consequat placerat sagittis. Aliquam ac sodales leo, id gravida dui. Nulla vel nulla et ipsum facilisis volutpat molestie sed ligula. Suspendisse congue dapibus lectus vel sagittis. Nullam posuere eu elit id commodo. Integer a leo nunc. Aliquam ac sapien eget metus commodo consectetur."#.to_string());
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn type_request() {
        let before = RequestMessage::new_type_request(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_response_compressed() {
        let before = ResponseMessage::new_text_response(
            21,
            Compression::LZW,
            "abcdefgh.".as_bytes().to_vec(),
        );
        let ret = simulate_server_compression(before.clone());

        let after = ResponseMessage::defragment(&ret, Compression::LZW).unwrap();

        assert_eq!(before, after);
    }
}
