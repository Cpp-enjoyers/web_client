#[cfg(test)]
mod web_client_tests {
    use std::{thread, time::Duration};

    use ap2024_unitn_cppenjoyers_drone::CppEnjoyersDrone;
    use common::Client;
    use crossbeam_channel::unbounded;
    use wg_2024::{controller::DroneCommand, drone::Drone};

    use crate::*;

    const MS500: Duration = Duration::from_millis(500);

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

    /*
        #[test]
        pub fn dr_ones_error() {
            // Client 1 channels
            let (c_send, c_recv) = unbounded();
            let (c87_send, c87_recv) = unbounded();
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
            //let (c_event_send, _) = unbounded();
           // let (_, c_command_recv) = unbounded();

            // Drone 11
            let neighbours11 = HashMap::from([
                (12, d12_send.clone()),
                (13, d13_send.clone()),
                (14, d14_send.clone()),
                (1, c_send.clone()),
                (87, c87_send.clone()),
            ]);
            let mut drone = dr_ones::Drone::new(
                11,
                d_event_send.clone(),
                d_command_recv.clone(),
                d_recv.clone(),
                neighbours11,
                0.0,
            );
            // Drone 12
            let neighbours12 = HashMap::from([(11, d_send.clone())]);
            let mut drone2 = dr_ones::Drone::new(
                12,
                d_event_send.clone(),
                d_command_recv.clone(),
                d12_recv.clone(),
                neighbours12,
                0.0,
            );
            // Drone 13
            let neighbours13 = HashMap::from([(11, d_send.clone()), (14, d14_send.clone())]);
            let mut drone3 = dr_ones::Drone::new(
                13,
                d_event_send.clone(),
                d_command_recv.clone(),
                d13_recv.clone(),
                neighbours13,
                0.0,
            );
            // Drone 14
            let neighbours14 = HashMap::from([(11, d_send.clone()), (13, d13_send.clone())]);
            let mut drone4 = dr_ones::Drone::new(
                14,
                d_event_send.clone(),
                d_command_recv.clone(),
                d14_recv.clone(),
                neighbours14,
                0.0,
            );

            // client 1
            // let neighbours1 = HashMap::from([(11, d_send.clone()), (12, d12_send.clone())]);
            // let mut client1 = WebBrowser::new(
            //     1,
            //     c_event_send.clone(),
            //     c1_command_recv,
            //     c_recv.clone(),
            //     neighbours1,
            // );

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
            // thread::spawn(move || {
            //     client1.run();
            // });

            //println!("client {} - {:?}", c_event_recv.recv().unwrap());
            //println!("client {} - {:?}", d_recv.recv().unwrap());
            println!("client {} - waiting for messages");


            d_send.send(packet::Packet::new_flood_request(
                SourceRoutingHeader::empty_route(),
                0,
                FloodRequest {
                    flood_id: 0,
                    initiator_id: 1,
                    path_trace: vec![(1, NodeType::Client)],
                },
            ));
            println!("client {} - ----------{:?}", c_recv.recv().unwrap());
            println!("client {} - ----------{:?}", c_recv.recv().unwrap());
            println!("client {} - ----------{:?}", c_recv.recv().unwrap());
            sleep(MS500);
            d_send.send(packet::Packet::new_flood_request(
                SourceRoutingHeader::empty_route(),
                0,
                FloodRequest {
                    flood_id: 0,
                    initiator_id: 87,
                    path_trace: vec![(87, NodeType::Client)],
                },
            ));

            c87_recv.recv().unwrap();
            println!("client {} - ----------{:?}", c87_recv.recv().unwrap());
            println!("client {} - ----------{:?}", c87_recv.recv().unwrap());



        }
    */

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
        let (c_send, c_recv) = unbounded();
        // Drone 11
        let (d_send, d_recv) = unbounded();
        // Drone 12
        let (d12_send, d12_recv) = unbounded();
        // Drone 13
        let (d13_send, d13_recv) = unbounded();
        // SC - needed to not make the drone crash
        let (_d_command_send, d_command_recv) = unbounded();
        let (_d12_command_send, d12_command_recv) = unbounded();
        let (c_event_send, _) = unbounded();
        let (c_command_send, c_command_recv) = unbounded();

        // Drone 11
        let neighbours11 = HashMap::from([(1, c_send.clone())]);
        let mut drone = CppEnjoyersDrone::new(
            11,
            unbounded().0,
            d_command_recv.clone(),
            d_recv.clone(),
            neighbours11,
            0.0,
        );
        // Drone 12
        let neighbours12 = HashMap::from([(13, d13_send.clone())]);
        let mut drone2 = CppEnjoyersDrone::new(
            12,
            unbounded().0,
            d12_command_recv.clone(),
            d12_recv.clone(),
            neighbours12,
            0.0,
        );
        // Drone 13
        let neighbours13 = HashMap::from([(12, d12_send.clone())]);
        let mut drone3 = CppEnjoyersDrone::new(
            13,
            unbounded().0,
            d_command_recv.clone(),
            d13_recv.clone(),
            neighbours13,
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

        sleep(MS500);
        thread::spawn(move || {
            client1.run();
        });

        _d12_command_send.send(DroneCommand::AddSender(1, c_send.clone()));

        sleep(MS500);

        c_command_send.send(ClientCommand::AddSender(12, d12_send.clone()));

        sleep(MS500);
        /*
        graph:
            1: [(11, Outgoing), (11, Incoming), (12, Outgoing), (12, Incoming)],
            11: [(1, Incoming), (1, Outgoing)],
            12: [(1, Incoming), (1, Outgoing), (13, Outgoing), (13, Incoming)],
            13: [(12, Incoming), (12, Outgoing)]
        }
        nodes type: {12: Drone, 1: Client, 11: Drone, 13: Drone}
        */
    }

    #[test]
    pub fn file_list_request() {
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

        let _ = c_command_send.send(ClientCommand::AskListOfFiles(21));

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
        // ACK
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            req.session_id,
            0,
        ));

        // response
        let data = web_messages::ResponseMessage::new_text_list_response(
            21,
            Compression::None,
            vec!["file1".to_string(), "file2".to_string()],
        )
        .fragment()
        .unwrap();
        assert_eq!(data.len(), 1);

        let _ = d_send.send(Packet::new_fragment(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            1 << 50,
            data[0].clone(),
        ));

        sleep(MS500);
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
        //println!("--{:?}", resp);

        if let ClientEvent::ListOfFiles(files, id) = resp {
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

        let _ = c_command_send.send(ClientCommand::AskServersTypes);

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

        if let ClientEvent::ServersTypes(map) = resp {
            assert_eq!(map, HashMap::from([(21, ServerType::FileServer)]));
        } else {
            assert!(false)
        }
    }

    #[test]
    pub fn file_request_no_media() {
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

        // receive flood request and send response
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

        // send ServerType command to drone
        let _ = c_command_send.send(ClientCommand::AskServersTypes);

        // receive type request from client
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        assert_eq!(req.session_id, 0);
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
        // send ACK
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            0,
            0,
        ));

        // send my type
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
            0,
            data[0].clone(),
        ));

        sleep(MS500);

        // client response to scl
        c_event_recv.recv().unwrap();
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 0,
                routing_header: SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );
        let resp = c_event_recv.recv().unwrap();
        //println!("--{:?}", resp);
        if let ClientEvent::ServersTypes(map) = resp {
            assert_eq!(map, HashMap::from([(21, ServerType::FileServer)]));
        } else {
            assert!(false)
        }

        let file = r#"Dolor molestiae debitis asperiores provident aut odit ratione. Sit totam officia dolores eos cumque blanditiis amet. Nesciunt voluptas voluptatem quas amet illum ipsam corrupti voluptate. Perspiciatis necessitatibus occaecati aliquid. Tempore id voluptas perferendis. Quis molestias enim veniam.
        Architecto enim sequi et sunt ut iusto repellendus. Voluptatem placeat iure veniam recusandae sit velit. Autem sed vel error et eaque aperiam deserunt. Vero iste quibusdam qui alias ullam qui et quo.
        Et eligendi voluptas ut nobis iste culpa est aliquam. Saepe quos commodi assumenda beatae voluptatem. Aliquam occaecati quia ut. Possimus aut officiis deleniti non. Qui voluptas enim nemo et.
        Ut expedita aut cum minima quo nostrum. Nemo distinctio non rem voluptatem reiciendis incidunt vel soluta. Voluptatem autem aut et alias sunt. Et et quo mollitia necessitatibus earum.
        Nihil qui odit cum temporibus. Alias sit quos est placeat. Non iste placeat voluptatem est ipsum aperiam recusandae nulla. Praesentium vel quis doloremque nemo quae. Et tempora beatae sunt corporis asperiores nesciunt."#.to_string();

        // send file command to client
        let _ = c_command_send.send(ClientCommand::RequestFile("file1".to_string(), 21));

        // receive file request from client
        let req = s_recv.recv().unwrap();
        assert_eq!(req.session_id, 1);
        //println!("****{:?}", req);
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
                content: web_messages::Request::Text(TextRequest::Text("file1".to_string()))
            }
        );
        // send ACK
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            1,
            0,
        ));

        // send my file
        let data = web_messages::ResponseMessage::new_text_response(21, Compression::None, file.clone())
            .fragment()
            .unwrap();
        let n_frags = data.len();
        let mut packet_id = PacketId::from_u64(1);

        for f in data {
            let _ = d_send.send(Packet::new_fragment(
                SourceRoutingHeader {
                    hop_index: 1,
                    hops: vec![21, 11, 1],
                },
                packet_id.get_session_id(),
                f,
            ));

           packet_id.increment_packet_id();

        }

        sleep(MS500);
        let mut packet_id = PacketId::from_u64(1);

        // ACKs to server
        for i in 0..n_frags{
            let _  = c_event_recv.recv().unwrap();
            //println!("{i}");
            assert_eq!(
                s_recv.recv().unwrap(),
                Packet {
                    session_id: packet_id.get_session_id(),
                    routing_header: SourceRoutingHeader {
                        hop_index: 2,
                        hops: vec![1, 11, 21]
                    },
                    pack_type: PacketType::Ack(Ack { fragment_index: i as u64})
                }
            );
            packet_id.increment_packet_id();

        }

        // client response
        let a= c_event_recv.recv().unwrap();
        //println!("-----{:?}", a);
        if let ClientEvent::FileFromClient(content, id) = a{
            assert_eq!(content, file);
            assert_eq!(id, 21);
        }
        else {
            assert!(false)
        }
    }

    #[test]
    pub fn media_request() {
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

        // receive flood request and send response
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

        // send ServerType command to drone
        let _ = c_command_send.send(ClientCommand::AskServersTypes);

        // receive type request from client
        //println!("{:?}", req);
        let req = s_recv.recv().unwrap();
        assert_eq!(req.session_id, 0);
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
        // send ACK
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            0,
            0,
        ));

        // send my type
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
            0,
            data[0].clone(),
        ));

        sleep(MS500);

        // client response to scl
        c_event_recv.recv().unwrap();
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 0,
                routing_header: SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );
        let resp = c_event_recv.recv().unwrap();
        //println!("--{:?}", resp);
        if let ClientEvent::ServersTypes(map) = resp {
            assert_eq!(map, HashMap::from([(21, ServerType::FileServer)]));
        } else {
            assert!(false)
        }

        let file = r#"Dolor molestiae debitis asperiores provident aut odit ratione. Sit totam officia dolores eos cumque blanditiis amet. Nesciunt voluptas voluptatem quas amet illum ipsam corrupti voluptate. Perspiciatis necessitatibus occaecati aliquid. Tempore id voluptas perferendis. Quis molestias enim veniam.
        Architecto enim sequi et sunt ut iusto repellendus. Voluptatem placeat iure veniam recusandae sit velit. Autem sed vel error et eaque aperiam deserunt. Vero iste quibusdam qui alias ullam qui et quo.
        Et eligendi voluptas ut nobis iste culpa est aliquam. Saepe quos commodi assumenda beatae voluptatem. Aliquam occaecati quia ut. Possimus aut officiis deleniti non. Qui voluptas enim nemo et.
        Ut expedita aut cum minima quo nostrum. Nemo distinctio non rem voluptatem reiciendis incidunt vel soluta. Voluptatem autem aut et alias sunt. Et et quo mollitia necessitatibus earum.
        Nihil qui odit cum temporibus. Alias sit quos est placeat. Non iste placeat voluptatem est ipsum aperiam recusandae nulla. Praesentium vel quis doloremque nemo quae. Et tempora beatae sunt corporis asperiores nesciunt."#.to_string();

        // send file command to client
        let _ = c_command_send.send(ClientCommand::RequestFile("file1".to_string(), 21));

        // receive file request from client
        let req = s_recv.recv().unwrap();
        assert_eq!(req.session_id, 1);
        //println!("****{:?}", req);
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
                content: web_messages::Request::Text(TextRequest::Text("file1".to_string()))
            }
        );
        // send ACK
        let _ = d_send.send(Packet::new_ack(
            SourceRoutingHeader {
                hop_index: 1,
                hops: vec![21, 11, 1],
            },
            1,
            0,
        ));

        // send my file
        let data = web_messages::ResponseMessage::new_text_response(21, Compression::None, file.clone())
            .fragment()
            .unwrap();
        let n_frags = data.len();
        let mut packet_id = PacketId::from_u64(1);

        for f in data {
            let _ = d_send.send(Packet::new_fragment(
                SourceRoutingHeader {
                    hop_index: 1,
                    hops: vec![21, 11, 1],
                },
                packet_id.get_session_id(),
                f,
            ));

           packet_id.increment_packet_id();

        }

        sleep(MS500);
        let mut packet_id = PacketId::from_u64(1);

        // ACKs to server
        for i in 0..n_frags{
            let _  = c_event_recv.recv().unwrap();
            //println!("{i}");
            assert_eq!(
                s_recv.recv().unwrap(),
                Packet {
                    session_id: packet_id.get_session_id(),
                    routing_header: SourceRoutingHeader {
                        hop_index: 2,
                        hops: vec![1, 11, 21]
                    },
                    pack_type: PacketType::Ack(Ack { fragment_index: i as u64})
                }
            );
            packet_id.increment_packet_id();

        }

        // client response
        let a= c_event_recv.recv().unwrap();
        //println!("-----{:?}", a);
        if let ClientEvent::FileFromClient(content, id) = a{
            assert_eq!(content, file);
            assert_eq!(id, 21);
        }
        else {
            assert!(false)
        }
    }


}

#[cfg(test)]
mod fragmentation_tests{
    use common::web_messages::{Compression, RequestMessage, ResponseMessage};

    use crate::Fragmentable;

    #[test]
    fn invalid_request_response(){
        let before = ResponseMessage::new_invalid_request_response(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn media_list_response(){
        let before = ResponseMessage::new_media_list_response(21, Compression::None, vec!["a, b, c".to_string(), "d".to_string()]);
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn media_response(){
        let before = ResponseMessage::new_media_response(21, Compression::None, r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed in lobortis ex. Fusce gravida pharetra lacus, sed ornare quam aliquam sit amet. In hac habitasse platea dictumst. Vestibulum tristique est et varius cursus. Donec tincidunt suscipit augue, eget porta enim porta at. Integer rutrum viverra dictum. Donec neque justo, euismod nec sem vel, porttitor lacinia nunc. Vestibulum sagittis, metus sed tempus tempus, lorem metus placerat velit, sit amet blandit arcu quam imperdiet est. Sed pulvinar erat ut diam bibendum euismod. Proin fermentum nec velit eget fermentum. Phasellus non dapibus urna, eget tempus neque. Aliquam id nibh sed nunc condimentum tempus ac et dui.
Etiam eget sollicitudin massa. Nam et sem sit amet sem facilisis vulputate quis sed ante. In vel mi ut nulla commodo facilisis ac eu orci. Suspendisse potenti. Cras sollicitudin volutpat diam ut porttitor. Nam non tellus eros. Mauris eu interdum augue, quis tincidunt odio."#.as_bytes().to_vec());
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn not_found_response(){
        let before = ResponseMessage::new_not_found_response(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_list_response(){
        let before = ResponseMessage::new_text_list_response(21, Compression::None, vec!["a".to_string(), "b".to_string()]);
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_response(){
        let before = ResponseMessage::new_text_response(21, Compression::None, r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed in lobortis ex. Fusce gravida pharetra lacus, sed ornare quam aliquam sit amet. In hac habitasse platea dictumst. Vestibulum tristique est et varius cursus. Donec tincidunt suscipit augue, eget porta enim porta at. Integer rutrum viverra dictum. Donec neque justo, euismod nec sem vel, porttitor lacinia nunc. Vestibulum sagittis, metus sed tempus tempus, lorem metus placerat velit, sit amet blandit arcu quam imperdiet est. Sed pulvinar erat ut diam bibendum euismod. Proin fermentum nec velit eget fermentum. Phasellus non dapibus urna, eget tempus neque. Aliquam id nibh sed nunc condimentum tempus ac et dui.
Etiam eget sollicitudin massa. Nam et sem sit amet sem facilisis vulputate quis sed ante. In vel mi ut nulla commodo facilisis ac eu orci. Suspendisse potenti. Cras sollicitudin volutpat diam ut porttitor. Nam non tellus eros. Mauris eu interdum augue, quis tincidunt odio."#.to_string());
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn type_response(){
        let before = ResponseMessage::new_type_response(21, Compression::None, common::slc_commands::ServerType::FileServer);
        let data = before.fragment().unwrap();
        let after = ResponseMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn media_list_request(){
        let before = RequestMessage::new_media_list_request(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn media_request(){
        let before = RequestMessage::new_media_request(21, Compression::None, r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed in lobortis ex. Fusce gravida pharetra lacus, sed ornare quam aliquam sit amet. In hac habitasse platea dictumst. Vestibulum tristique est et varius cursus. Donec tincidunt suscipit augue, eget porta enim porta at. Integer rutrum viverra dictum. Donec neque justo, euismod nec sem vel, porttitor lacinia nunc. Vestibulum sagittis, metus sed tempus tempus, lorem metus placerat velit, sit amet blandit arcu quam imperdiet est. Sed pulvinar erat ut diam bibendum euismod. Proin fermentum nec velit eget fermentum. Phasellus non dapibus urna, eget tempus neque. Aliquam id nibh sed nunc condimentum tempus ac et dui.
Etiam eget sollicitudin massa. Nam et sem sit amet sem facilisis vulputate quis sed ante. In vel mi ut nulla commodo facilisis ac eu orci. Suspendisse potenti. Cras sollicitudin volutpat diam ut porttitor. Nam non tellus eros. Mauris eu interdum augue, quis tincidunt odio."#.to_string());
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_list_request(){
        let before = RequestMessage::new_text_list_request(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

    #[test]
    fn text_request(){
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
    fn type_request(){
        let before = RequestMessage::new_type_request(21, Compression::None);
        let data = before.fragment().unwrap();
        let after = RequestMessage::defragment(&data, Compression::None).unwrap();

        assert_eq!(before, after);
    }

}