use common::slc_commands::{ClientCommand, ClientEvent, ServerEvent, ServerType};
// use common::flooder::Flooder;
// use common::RingBuffer;
use common::networking::flooder::Flooder;
use common::ring_buffer::RingBuffer;
use common::web_messages::*;
use common::Client;
use common::{slc_commands, web_messages};
use compression::lzw::LZWCompressor;
use compression::Compressor;
use petgraph::graph::Node;

use core::time;
use crossbeam_channel::{select, Receiver, Sender};
use petgraph::algo::astar;
use petgraph::prelude::{DiGraphMap, UnGraphMap};
use std::collections::HashMap;
use std::thread::sleep;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{self, *};

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

    fn defragment(v: &Vec<Fragment>, compr: Compression) -> Result<Self, SerializationError>
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
                    for i in 0..c.len() {
                        ret[i] = c[i];
                    }
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

    fn defragment(v: &Vec<Fragment>, compr: Compression) -> Result<Self, SerializationError>
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
                    for i in 0..c.len() {
                        ret[i] = c[i];
                    }
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

    fn defragment(v: &Vec<Fragment>, compr: Compression) -> Result<Self, SerializationError>
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

type SessionId = u64;
type RequestId = u16;

#[derive(Debug, Hash)]
struct PacketIdWrapper {
    session_id: SessionId, // only 48 LSB used
    req_id: RequestId,
}
impl PacketIdWrapper {
    fn new() -> Self {
        Self {
            session_id: 0,
            req_id: 0,
        }
    }

    fn from_u64(n: u64) -> Self {
        Self {
            session_id: n >> 16,
            req_id: (n & 0xFFFF) as u16,
        }
    }

    fn get_session_id(&self) -> u64 {
        self.session_id & 0x0000FFFFFFFFFFFF
    }

    fn get_request_id(&self) -> u16 {
        self.req_id
    }

    fn get_id(&self) -> u64 {
        self.session_id << 48 | self.req_id as u64
    }

    fn increment_session_id(&mut self) {
        self.session_id += 1;
        self.session_id &= 0x0000FFFFFFFFFFFF;
    }

    fn increment_request_id(&mut self) {
        self.session_id = 0; // new request has to reset the session id
        self.req_id += 1;
    }
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
    packet_id_counter: PacketIdWrapper,
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
        for (a, _) in &packet_send {
            initial_edges.push((id.clone(), a.clone()));
            initial_edges.push((a.clone(), id.clone()));
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
            packet_id_counter: PacketIdWrapper::new(),
            topology_graph: DiGraphMap::from_edges(initial_edges),
            nodes_type: HashMap::from([(id.clone(), GraphNodeType::Client)]),
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

                let current_hop: NodeId;
                match source_header.current_hop() {
                    Some(id) => current_hop = id,
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

        return (packet, None);
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
                                PacketIdWrapper::from_u64(packet.session_id).get_request_id();

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
                                PacketIdWrapper::from_u64(packet.session_id).get_request_id();

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
                                        .position(|f| f.fragment_index == fragment.fragment_index)
                                        .is_some()
                                    {
                                        println!("client {} - I received the same fragment multiple times, bug, ignoring the message", self.id);
                                        unreachable!()
                                    }

                                    req.incoming_messages.push(fragment.clone());

                                    let ack_dest = req.server_id.clone();
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
                        if self
                            .nodes_type
                            .iter()
                            .find(|(_, t)| matches!(t, GraphNodeType::Server))
                            .is_none()
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
        match astar(&self.topology_graph, self.id, |n| n == dest, |e| 1, |_| 0) {
            Some((_, path)) => Some(wg_2024::network::SourceRoutingHeader::with_first_hop(path)),
            None => None,
        }
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
                    self.packet_id_counter.get_id(),
                    f,
                );
                waiting_for_ack.push(p.clone());

                self.send_packet(p, channel);

                self.packet_id_counter.increment_session_id();
            }
        } else {
            for f in frags {
                let p = wg_2024::packet::Packet::new_fragment(
                    SourceRoutingHeader::empty_route(),
                    self.packet_id_counter.get_id(),
                    f,
                );
                waiting_for_flood.push(p.clone());
            }

            self.packet_id_counter.increment_session_id();
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
                        node_id.clone(),
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

#[cfg(test)]
mod client_tests {
    use std::{thread, time::Duration};

    use ap2024_unitn_cppenjoyers_drone::CppEnjoyersDrone;
    use common::Client;
    use crossbeam_channel::unbounded;
    use wg_2024::{controller::DroneCommand, drone::Drone};

    use crate::*;

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

        sleep(Duration::from_secs(1));
        thread::spawn(move || {
            client1.run();
        });

        sleep(Duration::from_secs(2));

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

            sleep(Duration::from_secs(1));
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
            sleep(Duration::from_secs(1));
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
        let (c_event_send, c_event_recv) = unbounded();
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

        sleep(Duration::from_secs(2));

        /*
        graph: {
            1: [(12, Outgoing), (12, Incoming), (11, Outgoing), (11, Incoming)],
            12: [(1, Incoming), (1, Outgoing), (11, Outgoing), (11, Incoming)],
            11: [(1, Incoming), (1, Outgoing), (12, Incoming), (12, Outgoing), (13, Outgoing), (13, Incoming), (14, Outgoing), (14, Incoming)],
            13: [(11, Incoming), (11, Outgoing), (14, Outgoing), (14, Incoming), (2, Incoming)],
            14: [(13, Incoming), (13, Outgoing), (11, Incoming), (11, Outgoing), (2, Incoming)],
            2: [(13, Incoming), (14, Incoming)]}
        }
        nodes type: {2: Client, 14: Drone, 11: Drone, 12: Drone, 13: Drone}

        client: 2 - graph: {
            2: [(14, Outgoing), (14, Incoming), (13, Outgoing), (13, Incoming)],
            14: [(2, Incoming), (2, Outgoing), (13, Outgoing), (13, Incoming), (11, Outgoing), (11, Incoming)],
            13: [(2, Incoming), (2, Outgoing), (14, Incoming), (14, Outgoing), (11, Outgoing), (11, Incoming)],
            11: [(13, Incoming), (13, Outgoing), (14, Incoming), (14, Outgoing), (12, Outgoing), (12, Incoming), (1, Incoming)],
            12: [(11, Incoming), (11, Outgoing), (1, Incoming)],
            1: [(12, Incoming), (11, Incoming)]
        }
        nodes type: {1: Client, 13: Drone, 11: Drone, 12: Drone, 14: Drone}
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

        sleep(Duration::from_secs(1));
        thread::spawn(move || {
            client1.run();
        });

        _d12_command_send.send(DroneCommand::AddSender(1, c_send.clone()));

        sleep(Duration::from_secs(2));

        c_command_send.send(ClientCommand::AddSender(12, d12_send.clone()));

        sleep(Duration::from_secs(2));
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

        sleep(Duration::from_secs(1));

        thread::spawn(move || {
            client1.run();
        });

        sleep(Duration::from_secs(1));
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

        sleep(Duration::from_secs(1));

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

        sleep(Duration::from_secs(1));
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

        sleep(Duration::from_secs(1));

        thread::spawn(move || {
            client1.run();
        });

        sleep(Duration::from_secs(1));
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

        sleep(Duration::from_secs(1));

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

        sleep(Duration::from_secs(1));
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
        sleep(Duration::from_secs(1));
        thread::spawn(move || {
            client1.run();
        });
        sleep(Duration::from_secs(1));
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
        sleep(Duration::from_secs(1));

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

        sleep(Duration::from_secs(1));

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
        let data = web_messages::ResponseMessage::new_text_response(21, Compression::None, file)
            .fragment()
            .unwrap();

        let n_frags = data.len();
        let mut i = 0;
        for f in data {
            let _ = d_send.send(Packet::new_fragment(
                SourceRoutingHeader {
                    hop_index: 1,
                    hops: vec![21, 11, 1],
                },
                1 << (22 + i) + 1,
                f,
            ));

            i += 1;
        }

        sleep(Duration::from_secs(1));

        // client response to scl
        println!("**---***{:?}", c_event_recv.recv().unwrap());
        assert_eq!(
            s_recv.recv().unwrap(),
            Packet {
                session_id: 1 << (22 + i) + 1,
                routing_header: SourceRoutingHeader {
                    hop_index: 2,
                    hops: vec![1, 11, 21]
                },
                pack_type: PacketType::Ack(Ack { fragment_index: 0 })
            }
        );
        let resp = c_event_recv.recv().unwrap();
        //println!("--{:?}", resp);
    }
}
