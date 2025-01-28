use common::slc_commands::{ClientCommand, ClientEvent, ServerType};
use core::time;
use crossbeam_channel::{select, Receiver, Sender};
use petgraph::algo::astar;
use petgraph::prelude::DiGraphMap;
use std::collections::HashMap;
use std::thread::sleep;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::*;

use common::networking::flooder::Flooder;
use common::ring_buffer::RingBuffer;
use common::web_messages::*;
use common::Client;
use common::{slc_commands, web_messages};
use compression::lzw::LZWCompressor;
use compression::Compressor;

mod packet_id;
use packet_id::{PacketId, RequestId};

mod web_client_test;

#[derive(Debug, Clone, PartialEq, Eq)]
enum GraphNodeType {
    Text,
    Media,
    Chat,
    Server,
    Client,
    Drone,
}
impl GraphNodeType {
    fn from_server_type(n: ServerType) -> Self {
        match n {
            ServerType::ChatServer => Self::Chat,
            ServerType::FileServer => Self::Text,
            ServerType::MediaServer => Self::Media,
        }
    }

    fn from_node_type(n: NodeType) -> Self {
        match n {
            NodeType::Client => Self::Client,
            NodeType::Drone => Self::Drone,
            NodeType::Server => Self::Server,
        }
    }
}

trait Fragmentable: Serializable {
    fn fragment(&self) -> Result<Vec<Fragment>, SerializationError>;

    fn defragment(v: &[Fragment], compr: Compression) -> Result<Self, SerializationError>
    where
        Self: Sized;
}

impl Fragmentable for RequestMessage {
    fn fragment(&self) -> Result<Vec<Fragment>, SerializationError> {
        let mut ret = vec![];
        let bytes = self.serialize()?;

        let chunks: std::slice::Chunks<'_, u8> = bytes.chunks(FRAGMENT_DSIZE);
        let n_frag = chunks.len();

        for c in chunks {
            let data = match <[u8; FRAGMENT_DSIZE]>::try_from(c) {
                Ok(arr) => arr,
                Err(_) => {
                    let mut ret: [u8; FRAGMENT_DSIZE] = [0; FRAGMENT_DSIZE];

                    ret[..c.len()].copy_from_slice(&c[..]);

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

        Ok(ret)
    }

    fn defragment(v: &[Fragment], compr: Compression) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let mut msg: Vec<u8> = vec![];
        for f in v {
            for i in 0..f.length {
                msg.push(f.data[i as usize]);
            }
        }

        let decompressed = match compr {
            Compression::LZW => {
                let compressed = <Vec<u16>>::deserialize(msg)?;
                LZWCompressor::new()
                    .decompress(compressed)
                    .map_err(|_| SerializationError)?
            }

            Compression::None => msg,
        };

        Self::deserialize(decompressed)
    }
}
impl Fragmentable for ResponseMessage {
    fn fragment(&self) -> Result<Vec<Fragment>, SerializationError> {
        let mut ret = vec![];
        let bytes = self.serialize()?;

        let chunks: std::slice::Chunks<'_, u8> = bytes.chunks(FRAGMENT_DSIZE);
        let n_frag = chunks.len();

        for c in chunks {
            let data = match <[u8; FRAGMENT_DSIZE]>::try_from(c) {
                Ok(arr) => arr,
                Err(_) => {
                    let mut ret: [u8; FRAGMENT_DSIZE] = [0; FRAGMENT_DSIZE];

                    ret[..c.len()].copy_from_slice(&c[..]);

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

        Ok(ret)
    }

    fn defragment(v: &[Fragment], compr: Compression) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let mut msg: Vec<u8> = vec![];
        for f in v {
            for i in 0..f.length {
                msg.push(f.data[i as usize]);
            }
        }

        let decompressed = match compr {
            Compression::LZW => {
                let compressed = <Vec<u16>>::deserialize(msg)?;
                LZWCompressor::new()
                    .decompress(compressed)
                    .map_err(|_| SerializationError)?
            }

            Compression::None => msg,
        };

        Self::deserialize(decompressed)
    }
}

const RING_BUFF_SZ: usize = 64;

#[derive(Debug, Clone)]
enum RequestType {
    ListOfFile(NodeId),
    ServersType,
    TextRequest(String, NodeId),
    MediaRequest(String, NodeId),
}


#[derive(Debug, Clone)]
struct Request {
    request_id: RequestId,
    server_id: NodeId,
    waiting_for_ack: Vec<Packet>, // stores the outgoing fragments that are still waiting for ACK
    incoming_messages: Vec<Fragment>, // stores the incoming fragments that compose the response of the query
    waiting_for_flood: Vec<Packet>, // stores the outgoing fragment for which I couldn't find a path or I received a NACK back instead of ACK
    compression: Compression,
    request_type: RequestType,
}
impl Request {
    fn new(
        request_id: RequestId,
        server_id: NodeId,
        waiting_for_ack: Vec<Packet>,
        waiting_for_flood: Vec<Packet>,
        compression: Compression,
        request_type: RequestType,
    ) -> Self {
        Self {
            request_id,
            server_id,
            waiting_for_ack,
            incoming_messages: vec![],
            waiting_for_flood,
            compression,
            request_type,
        }
    }
}
#[derive(Debug)]
pub struct WebBrowser {
    id: NodeId,
    //log_channel: String,
    controller_send: Sender<ClientEvent>,
    controller_recv: Receiver<ClientCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    flood_history: HashMap<NodeId, RingBuffer<u64>>,
    sequential_flood_id: u64,
    pending_requests: Vec<Request>,
    packet_id_counter: PacketId,
    topology_graph: DiGraphMap<NodeId, u64>,
    nodes_type: HashMap<NodeId, GraphNodeType>,
}

impl Flooder for WebBrowser {
    const NODE_TYPE: NodeType = NodeType::Client;

    fn get_id(&self) -> NodeId {
        self.id
    }

    fn get_neighbours(&self) -> impl ExactSizeIterator<Item = (&NodeId, &Sender<Packet>)> {
        self.packet_send.iter()
    }

    fn has_seen_flood(&self, flood_id: (NodeId, u64)) -> bool {
        match self.flood_history.get(&flood_id.0) {
            Some(set) => set.contains(&flood_id.1),
            None => false,
        }
    }

    fn insert_flood(&mut self, flood_id: (NodeId, u64)) {
        match self.flood_history.get_mut(&flood_id.0) {
            Some(set) => {
                set.insert(flood_id.1);
            }
            None => {
                let mut rb = RingBuffer::with_capacity(RING_BUFF_SZ);
                rb.insert(flood_id.1);
                self.flood_history.insert(flood_id.0, rb);
            }
        }
    }

    fn send_to_controller(&self, p: Packet) {
        self.controller_send
            .send(slc_commands::ClientEvent::Shortcut(p));
    }
}

impl Client for WebBrowser {
    fn new(
        id: NodeId,
        controller_send: Sender<ClientEvent>,
        controller_recv: Receiver<ClientCommand>,
        packet_recv: Receiver<Packet>,
        packet_send: HashMap<NodeId, Sender<Packet>>,
    ) -> Self {
        let mut initial_edges = vec![];
        for a in packet_send.keys() {
            initial_edges.push((id, a.clone()));
            initial_edges.push((a.clone(), id));
        }

        Self {
            id,
            //log_channel,
            controller_send,
            controller_recv,
            packet_recv,
            packet_send,
            flood_history: HashMap::from([(id, RingBuffer::with_capacity(RING_BUFF_SZ))]),
            sequential_flood_id: 0,
            pending_requests: Vec::new(),
            packet_id_counter: PacketId::new(),
            topology_graph: DiGraphMap::from_edges(initial_edges),
            nodes_type: HashMap::from([(id, GraphNodeType::Client)]),
        }
    }

    fn run(&mut self) {
        sleep(time::Duration::from_millis(100));

        self.start_flooding();

        loop {
            select! {
                recv(self.controller_recv) -> command => {
                    if let Ok(command) = command {
                        self.handle_command(command);
                    }
                },
                recv(self.packet_recv) -> packet => {
                    if let Ok(packet) = packet {
                        self.handle_packet(packet);
                    }
                },
            }
        }
    }
}

impl WebBrowser {
    fn shortcut(&self, packet: Packet) {
        //packet.routing_header.decrease_hop_index();
        let _ = self.controller_send.send(ClientEvent::Shortcut(packet));
    }

    fn send_nack(&self, packet: Packet, nack_type: NackType) {
        match packet.pack_type {
            // send original packet to simulation controller
            PacketType::Nack(_) | PacketType::Ack(_) | PacketType::FloodResponse(_) => {
                self.shortcut(packet);
            }
            // Send nack back
            PacketType::MsgFragment(f) => {
                let mut source_header: SourceRoutingHeader = match packet
                    .routing_header
                    .sub_route(0..packet.routing_header.hop_index)
                {
                    Some(sh) => sh,
                    None => {
                        println!("client {} - header is too short to be reversed to route the nack, dropping", self.id);
                        return;
                    }
                };
                source_header.reset_hop_index();
                source_header.reverse();
                source_header.hop_index = 1;

                if source_header.hops.len() < 2 {
                    println!("client {} - reversed header is too short to be used to route the nack, dropping", self.id);
                    return;
                }

                let current_hop = match source_header.current_hop() {
                    Some(id) => id,
                    None => {
                        println!(
                            "client {} - reversed header doesn't contain the next hop, dropping",
                            self.id
                        );
                        return;
                    }
                };

                match self.packet_send.get(&current_hop) {
                    Some(channel) => {
                        let nack: Packet = Packet::new_nack(
                            source_header,
                            packet.session_id,
                            Nack {
                                fragment_index: f.fragment_index,
                                nack_type,
                            },
                        );
                        let _ = self
                            .controller_send
                            .send(ClientEvent::PacketSent(nack.clone()));
                        let _ = channel.send(nack);
                    }

                    None => {
                        println!("client {} - Fragment has arrived to me but it can't go back. BUG, dropping", self.id);
                    }
                };
            }
            PacketType::FloodRequest(_) => {
                unreachable!()
            }
        }
    }

    /*
       Given a packet, id and destination it search for a path in the graph and returns
       an updated packet and an optional channel.
       if no path exist or the channel isn't open, the option is None
    */
    fn prepare_packet_routing(
        &self,
        mut packet: Packet,
        dest: NodeId,
    ) -> (Packet, Option<&Sender<Packet>>) {
        packet.routing_header = SourceRoutingHeader::empty_route();

        if let Some(routing_header) = self.source_routing_header_from_graph_search(dest) {
            if let Some(id) = routing_header.current_hop() {
                if let Some(channel) = self.packet_send.get(&id) {
                    match packet.pack_type {
                        PacketType::MsgFragment(_) | PacketType::Ack(_) | PacketType::Nack(_) => {
                            packet.routing_header = routing_header;
                        }

                        _ => {}
                    }

                    return (packet, Some(channel));
                }
            }
        }

        (packet, None)
    }

    fn handle_packet(&mut self, mut packet: Packet) {
        println!("client {} - handling packet: {:?}", self.id, packet);
        match packet.pack_type.clone() {
            PacketType::FloodRequest(mut req) => {
                let _ =
                    self.handle_flood_request(&packet.routing_header, packet.session_id, &mut req);
            }

            PacketType::FloodResponse(ref resp) => {
                let initiator: Option<&(NodeId, NodeType)> = resp.path_trace.first();

                if initiator.is_none() {
                    println!(
                        "client {} - Received a flood response with empty path trace, dropping",
                        self.id
                    );
                    return;
                }

                if initiator.unwrap().0 == self.id {
                    let mut prev: Option<(NodeId, NodeType)> = None;
                    for (id, node_type) in &resp.path_trace {
                        if let Some(from) = prev {
                            // this prevents A* to find path with client/server in the middle
                            if matches!(from.1, NodeType::Client)
                                | matches!(from.1, NodeType::Server)
                            {
                                self.topology_graph.add_edge(*id, from.0, 1);
                            } else if matches!(node_type, NodeType::Client)
                                | matches!(node_type, NodeType::Server)
                            {
                                self.topology_graph.add_edge(from.0, *id, 1);
                            } else {
                                self.topology_graph.add_edge(from.0, *id, 1);
                                self.topology_graph.add_edge(*id, from.0, 1);
                            }
                            self.nodes_type
                                .insert(*id, GraphNodeType::from_node_type(*node_type));
                        }
                        prev = Some((*id, *node_type));
                    }

                    println!(
                        "client: {} - graph: {:?} - nodes type: {:?}",
                        self.id, self.topology_graph, self.nodes_type
                    );

                    //self.try_resend_waiting_for_flood_packets();
                } else {
                    match packet.routing_header.next_hop() {
                        Some(next_hop_drone_id) => {
                            if let Some(channel) = self.packet_send.get(&next_hop_drone_id) {
                                packet.routing_header.increase_hop_index();
                                self.send_packet(packet, channel);
                            } else {
                                // I don't have the channel to forward the packet - SHORTCUT
                                self.shortcut(packet);
                            }
                        }

                        None => {
                            println!("client {} - Found a flood response with a corrupted routing header, I don't know who is the next hop nor, consequently, the original initiator to short shortcut this packet. Dropping", self.id);
                            unreachable!()
                        }
                    }
                }
            }

            PacketType::Ack(ref ack) => {
                match packet.routing_header.destination() {
                    Some(id) => {
                        // I'm the destination of the packet
                        if id == self.id {
                            let req_id =
                                PacketId::from_u64(packet.session_id).get_request_id();

                            match self
                                .pending_requests
                                .iter_mut()
                                .find(|req| req.request_id == req_id)
                            {
                                // I recognize this request_id
                                Some(req) => {
                                    match req
                                        .waiting_for_ack
                                        .iter()
                                        .position(|e| e.get_fragment_index() == ack.fragment_index)
                                    {
                                        Some(idx) => {
                                            req.waiting_for_ack.remove(idx);
                                        }

                                        None => {

                                            println!("client {} - I received an ack for a packet that has already been acknowledged, bug for me or for the sender", self.id);
                                        }
                                    }
                                }

                                None => {
                                    println!("client {} - I received an ack for an unknown req_id, dropping", self.id);
                                }
                            }
                        }
                        // I'm not the destination of this message, routing error, shortcut needed
                        else {
                            self.shortcut(packet);
                        }
                    }
                    // bad routing header, dropping
                    None => {
                        println!("client {} - Ack has a bad rounting header, I could not read the destination... dropping msg", self.id);
                    }
                }
            }

            PacketType::Nack(nack) => {}

            PacketType::MsgFragment(fragment) => {
                match packet.routing_header.destination() {
                    Some(id) => {
                        // I'm the destination of the packet
                        if id == self.id {
                            println!("-----packet id: {}", packet.session_id & 0xffff);
                            let req_id =
                                PacketId::from_u64(packet.session_id).get_request_id();

                            match self
                                .pending_requests
                                .iter_mut()
                                .find(|req| req.request_id == req_id)
                            {
                                // I recognize this open request
                                Some(req) => {
                                    let n_frags = fragment.total_n_fragments as usize;

                                    // check if a fragment has been already received
                                    if req
                                        .incoming_messages
                                        .iter()
                                        .any(|f| f.fragment_index == fragment.fragment_index)
                                    {
                                        println!("client {} - I received the same fragment multiple times, bug, ignoring the message", self.id);
                                        unreachable!()
                                    }

                                    req.incoming_messages.push(fragment.clone());

                                    let ack_dest = req.server_id;
                                    let tmp = req.clone();
                                    let received_frags = req.incoming_messages.len();

                                    // send ACK to acknowledge the packet
                                    self.send_ack(
                                        ack_dest,
                                        packet.session_id,
                                        fragment.fragment_index,
                                    );

                                    if received_frags == n_frags {
                                        // I have all the fragments
                                        self.complete_request(tmp);
                                    }
                                }

                                None => {
                                    println!("client {} - I received a fragment for req_id \"{}\" that it's unknown to me, dropping", self.id, req_id);
                                }
                            }
                        }
                        // I'm not the destination of this message, unexpected recipient, send nack back
                        else {
                            self.send_nack(packet, NackType::UnexpectedRecipient(self.id));
                        }
                    }
                    // bad routing header, dropping
                    None => {
                        println!("client {} - Message has a bad rounting header, I could not read the destination... dropping msg", self.id);
                    }
                }
            }
        }
    }

    fn send_ack(&self, server_id: NodeId, session_id: u64, fragment_index: u64) {
        let ack = Packet::new_ack(
            SourceRoutingHeader::empty_route(),
            session_id,
            fragment_index,
        );

        let (mut packet, opt_chn) = self.prepare_packet_routing(ack, server_id);

        if let Some(channel) = opt_chn {
            self.send_packet(packet, channel);
        } else {
            println!(
                "client {} - Can't find a path to the node, I need to shortcut ACK",
                self.id
            );
            packet.routing_header = SourceRoutingHeader::initialize(vec![self.id, server_id]);
            self.shortcut(packet);
        }
    }

    fn complete_request(&mut self, mut req: Request) {
        req.incoming_messages
            .sort_by(|f1, f2| f1.fragment_index.cmp(&f2.fragment_index));
        let content =
            web_messages::ResponseMessage::defragment(&req.incoming_messages, req.compression);

        if content.is_err() {
            println!("client {} - Cannot deserialize response, dropping", self.id);
            unreachable!()
        }

        match content.unwrap().content {
            Response::Generic(resp) => {
                match resp {
                    GenericResponse::Type(server_type) => {
                        self.nodes_type
                            .entry(req.server_id)
                            .and_modify(|t| *t = GraphNodeType::from_server_type(server_type));

                        // I discovered all the server type
                        if !self
                            .nodes_type
                            .iter()
                            .any(|(_, t)| matches!(t, GraphNodeType::Server))
                        {
                            let mut list = HashMap::new();
                            for (id, t) in &self.nodes_type {
                                match t {
                                    GraphNodeType::Chat => {
                                        list.insert(*id, ServerType::ChatServer);
                                    }
                                    GraphNodeType::Media => {
                                        list.insert(*id, ServerType::MediaServer);
                                    }
                                    GraphNodeType::Text => {
                                        list.insert(*id, ServerType::FileServer);
                                    }
                                    _ => {}
                                }
                            }
                            let _ = self.controller_send.send(ClientEvent::ServersTypes(list));
                        }
                    }
                    GenericResponse::InvalidRequest => {
                        let _ = self.controller_send.send(ClientEvent::UnsupportedRequest);
                    }
                    GenericResponse::NotFound => {
                        let _ = self.controller_send.send(ClientEvent::UnsupportedRequest);
                    }
                }
            }
            Response::Text(resp) => {
                match resp {
                    TextResponse::TextList(vec) => {
                        let _ = self
                            .controller_send
                            .send(ClientEvent::ListOfFiles(vec, req.server_id));
                    }
                    TextResponse::Text(file) => {
                        if file.contains("<img>") {
                            todo!()
                            //parsing ...
                            // ask for media
                            // store the file somewhere while waiting for media
                        } else {
                            // TODO maybe send to scl a vec<string> if media are not embedded
                            let _ = self
                                .controller_send
                                .send(ClientEvent::FileFromClient(file, req.server_id));
                        }
                    }
                }
            }
            Response::Media(resp) => {
                match resp {
                    MediaResponse::MediaList(vec) => {
                        todo!()
                        // check if medialist constains one of the media I need and ask for it
                    }
                    MediaResponse::Media(vec) => {
                        todo!()
                        // put the media together with its text file and, if no more media are needed, send to scl
                    }
                }
            }
        }

        // the request must exist inside the vector in order to reach this point, unwrap() should be safe
        self.pending_requests.remove(
            self.pending_requests
                .iter()
                .position(|r| r.request_id == req.request_id)
                .unwrap(),
        );
    }

    fn send_packet(&self, packet: Packet, channel: &Sender<Packet>) {
        let _ = self
            .controller_send
            .send(ClientEvent::PacketSent(packet.clone()));
        match channel.send(packet.clone()) {
            Ok(()) => println!("client {} - sent {:?}", self.id, packet.clone()),
            Err(e) => println!("client {} - {e}", self.id),
        }
    }

    fn handle_command(&mut self, command: ClientCommand) {
        println!("Handing command {:?}", command);
        match command {
            ClientCommand::AddSender(id, sender) => {
                self.packet_send.insert(id, sender);
                self.topology_graph.add_edge(self.id, id, 1);
                self.topology_graph.add_edge(id, self.id, 1);
                self.start_flooding();
            }

            ClientCommand::RemoveSender(id) => {
                self.packet_send.remove(&id);
                self.topology_graph.remove_node(id);
            }

            ClientCommand::AskListOfFiles(server_id) => {
                self.create_request(RequestType::ListOfFile(server_id));
            }

            ClientCommand::AskServersTypes => {
                self.create_request(RequestType::ServersType);
            }

            ClientCommand::RequestFile(filename, server_id) => {
                self.create_request(RequestType::TextRequest(filename, server_id))
            }

            ClientCommand::Shortcut(packet) => self.handle_packet(packet),

            _ => println!(
                "client {} - Chat messages are not implemented here!",
                self.id
            ),
        }
    }

    fn start_flooding(&mut self) {
        println!("client {} - starting flooding", self.id);

        let (p, _) = self.prepare_packet_routing(
            Packet::new_flood_request(
                SourceRoutingHeader::empty_route(),
                0, // sessionId is useless in flood requests and responses
                FloodRequest::initialize(self.sequential_flood_id, self.id, NodeType::Client),
            ),
            0, // useless for flood
        );

        self.flood_history.entry(self.id).and_modify(|ring| {
            ring.insert(self.sequential_flood_id);
        });

        for channel in self.packet_send.values() {
            self.send_packet(p.clone(), channel);
        }

        self.sequential_flood_id += 1;
    }

    fn source_routing_header_from_graph_search(&self, dest: NodeId) -> Option<SourceRoutingHeader> {
        // TODO change edge cost when present in the graph
        astar(&self.topology_graph, self.id, |n| n == dest, |_| 1, |_| 0)
            .map(|(_, path)| wg_2024::network::SourceRoutingHeader::with_first_hop(path))
    }

    fn add_request(
        &mut self,
        server_id: NodeId,
        compression: Compression,
        frags: Vec<Fragment>,
        request_type: RequestType,
    ) {
        println!("Adding request");
        let opt = self.source_routing_header_from_graph_search(server_id);

        let mut waiting_for_ack = vec![];
        let mut waiting_for_flood = vec![];

        if let Some(routing_header) = opt {
            let next_hop_id = routing_header
                .current_hop()
                .expect("Header created from a valid path is invalid. BUG");

            let channel = self
                .packet_send
                .get(&next_hop_id)
                .expect("Found inconsistency between graph and list of neighbours, BUG");

            for f in frags {
                let p = wg_2024::packet::Packet::new_fragment(
                    routing_header.clone(),
                    self.packet_id_counter.get_session_id(),
                    f,
                );
                waiting_for_ack.push(p.clone());

                self.send_packet(p, channel);

                self.packet_id_counter.increment_packet_id();
            }
        } else {
            for f in frags {
                let p = wg_2024::packet::Packet::new_fragment(
                    SourceRoutingHeader::empty_route(),
                    self.packet_id_counter.get_session_id(),
                    f,
                );
                waiting_for_flood.push(p.clone());
            }

            self.packet_id_counter.increment_packet_id();
            self.start_flooding();
        }

        self.pending_requests.push(Request::new(
            self.packet_id_counter.get_request_id(),
            server_id,
            waiting_for_ack,
            waiting_for_flood,
            compression,
            request_type,
        ));

        self.packet_id_counter.increment_request_id();
    }

    fn is_correct_server_type(&self, server_id: NodeId, requested_type: GraphNodeType) -> bool {
        self.nodes_type
            .get(&server_id)
            .is_some_and(|t| *t == requested_type)
    }

    fn create_request(&mut self, request_type: RequestType) {
        let compression: Compression = Compression::None; // TODO has to be chosen by scl or randomically

        match &request_type {
            RequestType::ListOfFile(server_id) => {
                let frags =
                    web_messages::RequestMessage::new_text_list_request(self.id, compression.clone())
                        .fragment()
                        .expect("Error during fragmentation. This can't happen. If it happens there is a bug somwhere");

                self.add_request(*server_id, compression, frags, request_type);
            }

            RequestType::ServersType => {
                let frags = RequestMessage::new_type_request(self.id, compression.clone())
                .fragment()
                .expect("Error during fragmentation. This can't happen. If it happens there is a bug somwhere");

                let server_list: Vec<NodeId> = self
                    .nodes_type
                    .iter()
                    .filter(|(_, t)| matches!(t, GraphNodeType::Server))
                    .map(|(id, _)| *id)
                    .collect();

                for node_id in server_list {
                    self.add_request(
                        node_id,
                        compression.clone(),
                        frags.clone(),
                        request_type.clone(),
                    );
                }
            }

            RequestType::MediaRequest(filename, server_id) => {
                if !self.is_correct_server_type(*server_id, GraphNodeType::Media) {
                    let _ = self.controller_send.send(ClientEvent::UnsupportedRequest);
                    return;
                }

                let frags =
                    web_messages::RequestMessage::new_media_request(self.id, compression.clone(), filename.clone())
                        .fragment()
                        .expect("Error during fragmentation. This can't happen. If it happens there is a bug somewhere");

                self.add_request(*server_id, compression, frags, request_type);
            }

            RequestType::TextRequest(filename, server_id) => {
                if !self.is_correct_server_type(*server_id, GraphNodeType::Text) {
                    let _ = self.controller_send.send(ClientEvent::UnsupportedRequest);
                    return;
                }

                let frags =
                    web_messages::RequestMessage::new_text_request(self.id, compression.clone(), filename.clone())
                        .fragment()
                        .expect("Error during fragmentation. This can't happen. If it happens there is a bug somwhere");

                self.add_request(*server_id, compression, frags, request_type);
            }
        }
    }
}

