use common::web_messages::{ResponseMessage, Serializable};
use compression::{lzw::LZWCompressor, Compressor};
use wg_2024::packet::{Fragment, FRAGMENT_DSIZE};

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
        let data = match <[u8; FRAGMENT_DSIZE]>::try_from(c) {
            Ok(arr) => arr,
            Err(_) => {
                let mut ret: [u8; FRAGMENT_DSIZE] = [0; FRAGMENT_DSIZE];

                ret[..c.len()].copy_from_slice(c);

                ret
            }
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

#[cfg(test)]
mod web_client_tests {
    use itertools::Itertools;
    use std::{hash::BuildHasher, thread, time::Duration, vec};

    use ap2024_unitn_cppenjoyers_drone::CppEnjoyersDrone;
    use common::Client;
    use common::{
        slc_commands::{WebClientCommand, WebClientEvent},
        web_messages::TextRequest,
    };
    use crossbeam_channel::unbounded;
    use petgraph::prelude::GraphMap;
    use wg_2024::drone::Drone;

    use crate::{web_client_test::simulate_server_compression, *};

    const MS500: Duration = Duration::from_millis(500);

    /// compares two graphmaps
    fn graphmap_eq<N, E, Ty, Ix>(a: &GraphMap<N, E, Ty, Ix>, b: &GraphMap<N, E, Ty, Ix>) -> bool
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
        /*
        for (a, b, c) in a_es.sorted_by(|a, b| a.partial_cmp(b).unwrap()) {
            print!("{a}, {b}, {c} - ");
        }
        println!("\n---");
        for (a, b, c) in b_es.sorted_by(|a, b| a.partial_cmp(b).unwrap()) {
            print!("{a}, {b}, {c} - ");
        }
        println!("\n-----");
        true
         */
    }

    #[test]
    pub fn my_flood_request_simple_top() {
        // Client 1 channels
        let (c_send, c_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();
        // Drone 12
        let (d12_send, d12_recv) = unbounded();
        // Drone 13
        let (d13_send, d13_recv) = unbounded();
        // Drone 14
        let (d14_send, d14_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (_d_command_send, d_command_recv) = unbounded();
        let (d_event_send, _d_event_rec) = unbounded();
        let (c_event_send, _) = unbounded();
        let (_, c_command_recv) = unbounded();

        // Drone 11
        let neighbours11 = HashMap::from([
            (12, d12_send.clone()),
            (13, d13_send.clone()),
            (14, d14_send.clone()),
            (1, c_send.clone()),
        ]);
        let mut drone = CppEnjoyersDrone::new(
            11,
            d_event_send.clone(),
            d_command_recv.clone(),
            d_recv.clone(),
            neighbours11,
            0.0,
        );
        // Drone 12
        let neighbours12 = HashMap::from([(11, d_send.clone())]);
        let mut drone2 = CppEnjoyersDrone::new(
            12,
            d_event_send.clone(),
            d_command_recv.clone(),
            d12_recv.clone(),
            neighbours12,
            0.0,
        );
        // Drone 13
        let neighbours13 = HashMap::from([(11, d_send.clone()), (14, d14_send.clone())]);
        let mut drone3 = CppEnjoyersDrone::new(
            13,
            d_event_send.clone(),
            d_command_recv.clone(),
            d13_recv.clone(),
            neighbours13,
            0.0,
        );
        // Drone 14
        let neighbours14 = HashMap::from([(11, d_send.clone()), (13, d13_send.clone())]);
        let mut drone4 = CppEnjoyersDrone::new(
            14,
            d_event_send.clone(),
            d_command_recv.clone(),
            d14_recv.clone(),
            neighbours14,
            0.0,
        );

        // client 1
        let neighbours1 = HashMap::from([(11, d_send.clone())]);
        let mut client1 = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv,
            c_recv.clone(),
            neighbours1,
        );

        // Spawn the drone's run method in a separate thread
        thread::spawn(move || {
            drone.run();
        });

        thread::spawn(move || {
            drone2.run();
        });

        thread::spawn(move || {
            drone3.run();
        });

        thread::spawn(move || {
            drone4.run();
        });

        sleep(MS500);
        thread::spawn(move || {
            client1.run();
        });

        sleep(MS500);

        /*
        graph: {
            1: [(11, Outgoing)],
            11: [(1, Incoming), (12, Outgoing), (13, Outgoing), (14, Outgoing)],
            12: [(11, Incoming)],
            13: [(11, Incoming), (14, Outgoing)],
            14: [(13, Incoming), (11, Incoming)]
        }
        nodes type: {12: Drone, 13: Drone, 14: Drone, 11: Drone}
        */
    }

    #[test]
    pub fn my_flood_request_complex_top() {
        // Client 1 channels
        let (c_send, c_recv) = unbounded();
        // Client 2 channels
        let (c2_send, c2_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();
        // Drone 12
        let (d12_send, d12_recv) = unbounded();
        // Drone 13
        let (d13_send, d13_recv) = unbounded();
        // Drone 14
        let (d14_send, d14_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (_d_command_send, d_command_recv) = unbounded();
        let (c_event_send, _) = unbounded();
        let (_, c1_command_recv) = unbounded();
        let (_, c2_command_recv) = unbounded();

        // Drone 11
        let neighbours11 = HashMap::from([
            (12, d12_send.clone()),
            (13, d13_send.clone()),
            (14, d14_send.clone()),
            (1, c_send.clone()),
        ]);
        let mut drone = CppEnjoyersDrone::new(
            11,
            unbounded().0,
            d_command_recv.clone(),
            d_recv.clone(),
            neighbours11,
            0.0,
        );
        // Drone 12
        let neighbours12 = HashMap::from([(1, c_send.clone()), (11, d_send.clone())]);
        let mut drone2 = CppEnjoyersDrone::new(
            12,
            unbounded().0,
            d_command_recv.clone(),
            d12_recv.clone(),
            neighbours12,
            0.0,
        );
        // Drone 13
        let neighbours13 = HashMap::from([
            (11, d_send.clone()),
            (14, d14_send.clone()),
            (2, c2_send.clone()),
        ]);
        let mut drone3 = CppEnjoyersDrone::new(
            13,
            unbounded().0,
            d_command_recv.clone(),
            d13_recv.clone(),
            neighbours13,
            0.0,
        );
        // Drone 14
        let neighbours14 = HashMap::from([
            (11, d_send.clone()),
            (13, d13_send.clone()),
            (2, c2_send.clone()),
        ]);
        let mut drone4 = CppEnjoyersDrone::new(
            14,
            unbounded().0,
            d_command_recv.clone(),
            d14_recv.clone(),
            neighbours14,
            0.0,
        );

        // client 1
        let neighbours1 = HashMap::from([(11, d_send.clone()), (12, d12_send.clone())]);
        let mut client1 = WebBrowser::new(
            1,
            c_event_send.clone(),
            c1_command_recv,
            c_recv.clone(),
            neighbours1,
        );

        // client 2
        let neighbours2 = HashMap::from([(13, d13_send.clone()), (14, d14_send.clone())]);
        let mut client2 = WebBrowser::new(
            2,
            c_event_send.clone(),
            c2_command_recv,
            c2_recv.clone(),
            neighbours2,
        );

        // Spawn the drone's run method in a separate thread
        thread::spawn(move || {
            drone.run();
        });

        thread::spawn(move || {
            drone2.run();
        });

        thread::spawn(move || {
            drone3.run();
        });

        thread::spawn(move || {
            drone4.run();
        });

        thread::spawn(move || {
            client1.run();
        });

        thread::spawn(move || {
            client2.run();
        });

        println!("waiting for messages");

        sleep(MS500);

        /*
        graph: {
            1: [(12, Outgoing), (12, Incoming), (11, Outgoing), (11, Incoming)],
            12: [(1, Incoming), (1, Outgoing), (11, Outgoing), (11, Incoming)],
            11: [(1, Incoming), (1, Outgoing), (12, Incoming), (12, Outgoing), (13, Outgoing), (13, Incoming), (14, Outgoing), (14, Incoming)],
            13: [(11, Incoming), (11, Outgoing), (14, Outgoing), (14, Incoming), (2, Outgoing)],
            14: [(13, Incoming), (13, Outgoing), (11, Incoming), (11, Outgoing), (2, Outgoing)],
            2: [(13, Incoming), (14, Incoming)]}
        }
        nodes type: {1: Client, 2: Client, 14: Drone, 11: Drone, 12: Drone, 13: Drone}

        client: 2 - graph: {
            2: [(14, Outgoing), (14, Incoming), (13, Outgoing), (13, Incoming)],
            14: [(2, Incoming), (2, Outgoing), (13, Outgoing), (13, Incoming), (11, Outgoing), (11, Incoming)],
            13: [(2, Incoming), (2, Outgoing), (14, Incoming), (14, Outgoing), (11, Outgoing), (11, Incoming)],
            11: [(13, Incoming), (13, Outgoing), (14, Incoming), (14, Outgoing), (12, Outgoing), (12, Incoming), (1, Incoming)],
            12: [(11, Incoming), (11, Outgoing), (1, Outgoing)],
            1: [(12, Incoming), (11, Incoming)]
        }
        nodes type: {2: Client, 1: Client, 13: Drone, 11: Drone, 12: Drone, 14: Drone}
        */
    }

    #[test]
    pub fn add_neighbour() {
        // Client 1 channels
        let (_c_send, c_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();

        let (c_event_send, _) = unbounded();
        let (_c_command_send, c_command_recv) = unbounded();

        let mut client = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv.clone(),
            c_recv,
            HashMap::new(),
        );

        sleep(MS500);

        client.handle_command(WebClientCommand::AddSender(11, d_send.clone()));

        assert_eq!(
            d_recv.recv().unwrap(),
            Packet::new_flood_request(
                SourceRoutingHeader::empty_route(),
                0,
                FloodRequest::initialize(0, 1, NodeType::Client)
            )
        );
    }

    #[test]
    pub fn file_list_scl_command() {
        // client 1 <--> 11 <--> 21 server

        // Client 1 channels
        let (c_send, c_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (_d_command_send, d_command_recv) = unbounded();
        let (c_event_send, c_event_recv) = unbounded();
        let (c_command_send, c_command_recv) = unbounded();
        // Server
        let (s_send, s_recv) = unbounded();

        // Drone 11
        let neighbours11 = HashMap::from([(1, c_send.clone()), (21, s_send.clone())]);
        let mut drone = CppEnjoyersDrone::new(
            11,
            unbounded().0,
            d_command_recv.clone(),
            d_recv.clone(),
            neighbours11,
            0.0,
        );

        // client 1
        let neighbours1 = HashMap::from([(11, d_send.clone())]);
        let mut client1 = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv,
            c_recv.clone(),
            neighbours1,
        );

        thread::spawn(move || {
            drone.run();
        });

        sleep(MS500);

        thread::spawn(move || {
            client1.run();
        });

        sleep(MS500);
        c_event_recv.recv().unwrap();

        let _flood_request = s_recv.recv().unwrap();
        let flood_response = Packet::new_flood_response(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            0,
            FloodResponse {
                flood_id: 0,
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (21, NodeType::Server),
                ],
            },
        );

        let _ = d_send.send(flood_response);

        sleep(MS500);

        let _ = c_command_send.send(WebClientCommand::AskListOfFiles(21));

        // receive request
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
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

        // ACK (sent later)
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            0,
        ));
        let _ = d_send.send(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            1 << 50,
            data[0].clone(),
        ));

        sleep(MS500);
        // ack from client
        c_event_recv.recv().unwrap();

        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 1 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        let resp = c_event_recv.recv().unwrap();
        println!("--{:?}", resp);

        if let WebClientEvent::ListOfFiles(files, id) = resp {
            assert_eq!(files, vec!["file1".to_string(), "file2".to_string()]);
            assert_eq!(id, 21);
        } else {
            assert!(false)
        }
    }

    #[test]
    pub fn file_list_scl_command_with_nack_dropped() {
        // client 1 <--> 11 <--> 21 server

        // Client 1 channels
        let (c_send, c_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (_d_command_send, d_command_recv) = unbounded();
        let (c_event_send, c_event_recv) = unbounded();
        let (c_command_send, c_command_recv) = unbounded();
        // Server
        let (s_send, s_recv) = unbounded();

        // Drone 11
        let neighbours11 = HashMap::from([(1, c_send.clone()), (21, s_send.clone())]);
        let mut drone = CppEnjoyersDrone::new(
            11,
            unbounded().0,
            d_command_recv.clone(),
            d_recv.clone(),
            neighbours11,
            0.0,
        );

        // client 1
        let neighbours1 = HashMap::from([(11, d_send.clone())]);
        let mut client1 = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv,
            c_recv.clone(),
            neighbours1,
        );

        thread::spawn(move || {
            drone.run();
        });

        sleep(MS500);

        thread::spawn(move || {
            client1.run();
        });

        sleep(MS500);
        c_event_recv.recv().unwrap();

        let _flood_request = s_recv.recv().unwrap();
        let flood_response = Packet::new_flood_response(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            0,
            FloodResponse {
                flood_id: 0,
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (21, NodeType::Server),
                ],
            },
        );

        let _ = d_send.send(flood_response);

        sleep(MS500);

        let _ = c_command_send.send(WebClientCommand::AskListOfFiles(21));

        // receive request
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
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
        let _ = d_send.send(nack).unwrap();

        // receive request second time
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
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

        // ACK (sent later)
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            0,
        ));
        let _ = d_send.send(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            1 << 50,
            data[0].clone(),
        ));

        sleep(MS500);
        // ack from client
        c_event_recv.recv().unwrap();

        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 1 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        let resp = c_event_recv.recv().unwrap();
        println!("--{:?}", resp);

        if let WebClientEvent::ListOfFiles(files, id) = resp {
            assert_eq!(files, vec!["file1".to_string(), "file2".to_string()]);
            assert_eq!(id, 21);
        } else {
            assert!(false)
        }
    }

    #[test]
    pub fn file_list_scl_command_with_4_nack_dropped() {
        // client 1 <--> 11 <--> 21 server

        // Client 1 channels
        let (c_send, c_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (_d_command_send, d_command_recv) = unbounded();
        let (c_event_send, c_event_recv) = unbounded();
        let (c_command_send, c_command_recv) = unbounded();
        // Server
        let (s_send, s_recv) = unbounded();

        // Drone 11
        let neighbours11 = HashMap::from([(1, c_send.clone()), (21, s_send.clone())]);
        let mut drone = CppEnjoyersDrone::new(
            11,
            unbounded().0,
            d_command_recv.clone(),
            d_recv.clone(),
            neighbours11,
            0.0,
        );

        // client 1
        let neighbours1 = HashMap::from([(11, d_send.clone())]);
        let mut client1 = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv,
            c_recv.clone(),
            neighbours1,
        );

        thread::spawn(move || {
            drone.run();
        });

        sleep(MS500);

        thread::spawn(move || {
            client1.run();
        });

        sleep(MS500);
        c_event_recv.recv().unwrap();

        let _flood_request = s_recv.recv().unwrap();
        let flood_response = Packet::new_flood_response(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            0,
            FloodResponse {
                flood_id: 0,
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (21, NodeType::Server),
                ],
            },
        );

        let _ = d_send.send(flood_response);

        sleep(MS500);

        let _ = c_command_send.send(WebClientCommand::AskListOfFiles(21));

        // receive request
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        c_event_recv.recv().unwrap();

        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );

        // --------

        // NACK 1 (dropped)
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
        let _ = d_send.send(nack).unwrap();
        // receive request second time
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        c_event_recv.recv().unwrap();

        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );

        // --------

        // NACK 2 (dropped)
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
        let _ = d_send.send(nack).unwrap();
        // receive request third time
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        c_event_recv.recv().unwrap();

        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );

        // --------

        // NACK 3 (dropped)
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
        let _ = d_send.send(nack).unwrap();
        // receive request fourth time
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
        c_event_recv.recv().unwrap();

        assert_eq!(
            req_defrag,
            RequestMessage {
                compression_type: Compression::None,
                source_id: 1,
                content: web_messages::Request::Text(TextRequest::TextList)
            }
        );

        // --------

        // NACK 4 (dropped)
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
        let _ = d_send.send(nack).unwrap();
        // receive request fifth time
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        let mut data = Vec::new();
        match req.pack_type {
            PacketType::MsgFragment(f) => data.push(f),
            _ => {}
        };
        let req_defrag = RequestMessage::defragment(&data, Compression::None).unwrap();
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

        // ACK (sent later)
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            0,
        ));
        let _ = d_send.send(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            1 << 50,
            data[0].clone(),
        ));

        sleep(MS500);
        // ack from client
        c_event_recv.recv().unwrap();

        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 1 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        let resp = c_event_recv.recv().unwrap();
        println!("--{:?}", resp);

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

        // Client 1 channels
        let (c_send, c_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (_d_command_send, d_command_recv) = unbounded();
        let (c_event_send, c_event_recv) = unbounded();
        let (c_command_send, c_command_recv) = unbounded();
        // Server
        let (s_send, s_recv) = unbounded();

        // Drone 11
        let neighbours11 = HashMap::from([(1, c_send.clone()), (21, s_send.clone())]);
        let mut drone = CppEnjoyersDrone::new(
            11,
            unbounded().0,
            d_command_recv.clone(),
            d_recv.clone(),
            neighbours11,
            0.0,
        );

        // client 1
        let neighbours1 = HashMap::from([(11, d_send.clone())]);
        let mut client1 = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv,
            c_recv.clone(),
            neighbours1,
        );

        thread::spawn(move || {
            drone.run();
        });

        sleep(MS500);

        thread::spawn(move || {
            client1.run();
        });

        sleep(MS500);
        c_event_recv.recv().unwrap();

        let _flood_request = s_recv.recv().unwrap();
        let flood_response = Packet::new_flood_response(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            0,
            FloodResponse {
                flood_id: 0,
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (21, NodeType::Server),
                ],
            },
        );

        let _ = d_send.send(flood_response);

        sleep(MS500);

        let _ = c_command_send.send(WebClientCommand::AskServersTypes);

        // receive request
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
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
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
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

        let _ = d_send.send(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            3 << 50,
            data[0].clone(),
        ));

        sleep(MS500);
        c_event_recv.recv().unwrap();

        // control ACK from client
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 3 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        // control client response to scl
        let resp = c_event_recv.recv().unwrap();
        //println!("--{:?}", resp);

        if let WebClientEvent::ServersTypes(map) = resp {
            assert_eq!(map, HashMap::from([(21, ServerType::FileServer)]));
        } else {
            assert!(false)
        }
    }

    #[test]
    pub fn server_type_request_nack_error_in_routing() {
        // client 1 <--> 11 <--> 21 server

        // Client 1 channels
        let (c_send, c_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (_d_command_send, d_command_recv) = unbounded();
        let (c_event_send, c_event_recv) = unbounded();
        let (c_command_send, c_command_recv) = unbounded();
        // Server
        let (s_send, s_recv) = unbounded();

        // Drone 11
        let neighbours11 = HashMap::from([(1, c_send.clone()), (21, s_send.clone())]);
        let mut drone = CppEnjoyersDrone::new(
            11,
            unbounded().0,
            d_command_recv.clone(),
            d_recv.clone(),
            neighbours11,
            0.0,
        );

        // client 1
        let neighbours1 = HashMap::from([(11, d_send.clone())]);
        let mut client1 = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv,
            c_recv.clone(),
            neighbours1,
        );

        thread::spawn(move || {
            drone.run();
        });

        sleep(MS500);

        thread::spawn(move || {
            client1.run();
        });

        sleep(MS500);
        c_event_recv.recv().unwrap();

        let _flood_request = s_recv.recv().unwrap();
        let flood_response = Packet::new_flood_response(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            0,
            FloodResponse {
                flood_id: 0,
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (21, NodeType::Server),
                ],
            },
        );
        let _ = d_send.send(flood_response);

        sleep(MS500);

        let _ = c_command_send.send(WebClientCommand::AskServersTypes);

        // send fake nack with problematic node id = 11
        let req_to_ignore = s_recv.recv().unwrap();
        let nack = Packet::new_nack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            req_to_ignore.session_id,
            Nack {
                fragment_index: 0,
                nack_type: NackType::ErrorInRouting(11),
            },
        );
        d_send.send(nack).unwrap();

        // floodrequest since now the client has no path to 21
        let _flood_request = s_recv.recv().unwrap();
        let flood_response = Packet::new_flood_response(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            0,
            FloodResponse {
                flood_id: 0,
                path_trace: vec![
                    (1, NodeType::Client),
                    (11, NodeType::Drone),
                    (21, NodeType::Server),
                ],
            },
        );
        let _ = d_send.send(flood_response);

        // receive request
        let req = s_recv.recv().unwrap();

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
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
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

        let _ = d_send.send(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            3 << 50,
            data[0].clone(),
        ));

        sleep(MS500);
        c_event_recv.recv().unwrap();

        // control ACK from client
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 3 << 50,
                routing_header: SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );

        // control client response to scl
        c_event_recv.recv().unwrap();
        c_event_recv.recv().unwrap();
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
        // Client 1 channels
        let (_c_send, c_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (c_event_send, c_event_recv) = unbounded();
        let (_c_command_send, c_command_recv) = unbounded();
        // Server
        let (s_send, s_recv) = unbounded();

        let mut client = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv.clone(),
            c_recv.clone(),
            HashMap::from([(21, s_send.clone())]),
        );
        client.topology_graph = GraphMap::from_edges([(1, 21, 1), (21, 1, 1)]);
        client.nodes_type = HashMap::from([(21, GraphNodeType::Media)]);
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
                TextMediaResponse::new(("file1.html".to_string(), "<html><h1>ciao</h1></html>".as_bytes().to_vec()), vec![]),
                21
            )
        );
    }

    #[test]
    pub fn file_request_one_media() {
        // Client 1 channels
        let (_c_send, c_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (c_event_send, c_event_recv) = unbounded();
        let (_c_command_send, c_command_recv) = unbounded();
        // Server
        let (s_send, s_recv) = unbounded();

        let mut client = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv.clone(),
            c_recv.clone(),
            HashMap::from([(21, s_send.clone())]),
        );
        client.topology_graph = GraphMap::from_edges([(1, 21, 1), (21, 1, 1)]);
        client.nodes_type = HashMap::from([(21, GraphNodeType::Media)]);
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
            request_type: RequestType::Text("file1.html".to_string(), 21),
            response_is_complete: true,
        };

        client.pending_requests.push(req);
        client.try_complete_request();

        assert_eq!(
            client.stored_files.get("file1.html").unwrap(),
            &"<html><img src=\"a/b/c/media.jpg\"/>".as_bytes().to_vec()
        );
        assert!(client.stored_files.get("a/b/c/media.jpg").is_none());
        assert_eq!(
            client
                .text_media_map
                .get(&(21, "file1.html".to_string()))
                .unwrap(),
            &vec!["a/b/c/media.jpg".to_string()]
        );
        assert_eq!(
            client.media_owner,
            HashMap::from([("a/b/c/media.jpg".to_string(), None)])
        );
        assert_eq!(
            client.media_request_left,
            HashMap::from([("a/b/c/media.jpg".to_string(), 1)])
        );

        // media file list request
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 21]),
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
            client
                .media_owner
                .get(&"a/b/c/media.jpg".to_string())
                .unwrap(),
            &None
        );
        assert!(client
            .stored_files
            .get(&"a/b/c/media.jpg".to_string())
            .is_none());
        assert_eq!(
            client
                .media_request_left
                .get(&"a/b/c/media.jpg".to_string())
                .unwrap(),
            &1
        );

        client.try_complete_request();

        assert_eq!(
            client
                .media_owner
                .get(&"a/b/c/media.jpg".to_string())
                .unwrap(),
            &Some(21)
        );
        assert!(client
            .stored_files
            .get(&"a/b/c/media.jpg".to_string())
            .is_none());
        assert_eq!(
            client
                .media_request_left
                .get(&"a/b/c/media.jpg".to_string())
                .unwrap(),
            &0
        );

        assert_eq!(
            client.pending_requests.get(0).unwrap().request_type,
            RequestType::Media("a/b/c/media.jpg".to_string(), 21)
        );
        s_recv.recv().unwrap();
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 21]),
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
        // ack
        client.pending_requests.get_mut(0).unwrap().waiting_for_ack = HashMap::new();

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

        // remove packetsent event from queue
        c_event_recv.recv().unwrap();
        c_event_recv.recv().unwrap();
        c_event_recv.recv().unwrap();
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

        assert!(client.media_owner.is_empty());
        assert!(client.media_request_left.is_empty());
        assert!(client.pending_requests.is_empty());
        assert!(client.stored_files.is_empty());
    }

/*
    #[test]
    pub fn file_request_three_media_third_unavailable() {
        todo!();
        // Client 1 channels
        let (_c_send, c_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (c_event_send, c_event_recv) = unbounded();
        let (_c_command_send, c_command_recv) = unbounded();
        // Server
        let (s_send, s_recv) = unbounded();

        let mut client = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv.clone(),
            c_recv.clone(),
            HashMap::from([(21, s_send.clone())]),
        );
        client.topology_graph = GraphMap::from_edges([(1, 21, 1), (21, 1, 1)]);
        client.nodes_type = HashMap::from([(21, GraphNodeType::Media)]);
        client.packet_id_counter = PacketId::from_u64(1);

        let html = "<html><img src=\"a/b/c/media.jpg\"/>fieub<img src=\"a/media2.jpg\"/>fieub<img src=\"c/media3.jpg\"/>fieub";
        let html_response = ResponseMessage::new_text_response(
            21,
            Compression::LZW,
            html.as_bytes().to_vec(),
        );

        let ret = simulate_server_compression(html_response.clone());
        let req = WebBrowserRequest {
            request_id: 0,
            server_id: 21,
            waiting_for_ack: HashMap::new(),
            incoming_messages: ret,
            compression: Compression::LZW,
            request_type: RequestType::Text("file1.html".to_string(), 21),
            response_is_complete: true,
        };

        client.pending_requests.push(req);
        client.try_complete_request();

        assert_eq!(
            client.stored_files.get("file1.html").unwrap(),
            &html.as_bytes().to_vec(),

        );
        assert!(client.stored_files.get("a/b/c/media.jpg").is_none());
        assert!(client.stored_files.get("a/media2.jpg").is_none());
        assert!(client.stored_files.get("c/media3.jpg").is_none());
        assert_eq!(
            client
                .text_media_map
                .get(&(21, "file1.html".to_string()))
                .unwrap(),
            &vec!["a/b/c/media.jpg".to_string(), "a/media2.jpg".to_string(), "c/media3.jpg".to_string()]
        );
        assert_eq!(
            client.media_owner,
            HashMap::from([("a/b/c/media.jpg".to_string(), None), ("a/media2.jpg".to_string(), None), ("c/media3.jpg".to_string(), None)])
        );
        assert_eq!(
            client.media_request_left,
            HashMap::from([("a/b/c/media.jpg".to_string(), 1), ("a/media2.jpg".to_string(), 1), ("c/media3.jpg".to_string(), 1)])
        );

        // media file list request
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 21]),
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
            client
                .media_owner
                .get(&"a/b/c/media.jpg".to_string())
                .unwrap(),
            &None
        );
        assert!(client
            .stored_files
            .get(&"a/b/c/media.jpg".to_string())
            .is_none());
        assert_eq!(
            client
                .media_request_left
                .get(&"a/b/c/media.jpg".to_string())
                .unwrap(),
            &1
        );

        client.try_complete_request();

        assert_eq!(
            client
                .media_owner
                .get(&"a/b/c/media.jpg".to_string())
                .unwrap(),
            &Some(21)
        );
        assert!(client
            .stored_files
            .get(&"a/b/c/media.jpg".to_string())
            .is_none());
        assert_eq!(
            client
                .media_request_left
                .get(&"a/b/c/media.jpg".to_string())
                .unwrap(),
            &0
        );

        assert_eq!(
            client.pending_requests.get(0).unwrap().request_type,
            RequestType::Media("a/b/c/media.jpg".to_string(), 21)
        );
        s_recv.recv().unwrap();
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet::new_fragment(
                SourceRoutingHeader::with_first_hop(vec![1, 21]),
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
        // ack
        client.pending_requests.get_mut(0).unwrap().waiting_for_ack = HashMap::new();

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

        // remove packetsent event from queue
        c_event_recv.recv().unwrap();
        c_event_recv.recv().unwrap();
        c_event_recv.recv().unwrap();
        c_event_recv.recv().unwrap();
        assert_eq!(
            c_event_recv.recv().unwrap(),
            ClientEvent::FileFromClient(
                TextMediaResponse::new((html_file), media_files)
                vec![
                    "<html><img src=\"a/b/c/media.jpg\"/>".as_bytes().to_vec(),
                    vec![1, 2, 3, 4, 5]
                ],
                21
            )
        );

        assert!(client.media_owner.is_empty());
        assert!(client.media_request_left.is_empty());
        assert!(client.pending_requests.is_empty());
        assert!(client.stored_files.is_empty());
    }

 */
    #[test]
    fn file_parsing() {
        let (_c, c_recv) = unbounded();
        let (c_event_send, _b) = unbounded();
        let (_a, c_command_recv) = unbounded();

        let client = WebBrowser::new(
            1,
            c_event_send.clone(),
            c_command_recv.clone(),
            c_recv.clone(),
            HashMap::new(),
        );

        assert_eq!(
            WebBrowser::get_media_inside_text_file(&"suhbefuiwfbwob".to_string()),
            Vec::<String>::new()
        );
        assert_eq!(
            WebBrowser::get_media_inside_text_file(
                &"-----------<img src=\"youtube.com\"\\>".to_string()
            ),
            vec!["youtube.com".to_string()]
        );
        assert_eq!(
            WebBrowser::get_media_inside_text_file(
                &"-----------<img src=\"/usr/tmp/folder/subfolder/pic.jpg\"\\>".to_string()
            ),
            vec!["/usr/tmp/folder/subfolder/pic.jpg".to_string()]
        );
    }
}

#[cfg(test)]
mod fragmentation_tests {
    use common::web_messages::{Compression, RequestMessage, ResponseMessage, Serializable};
    use compression::{lzw::LZWCompressor, Compressor};
    use wg_2024::packet::{Fragment, FRAGMENT_DSIZE};

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
