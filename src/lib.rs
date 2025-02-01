use common::slc_commands::{ClientCommand, ClientEvent, ServerType};
use core::time;
use crossbeam_channel::{select_biased, Receiver, Sender};
use petgraph::algo::astar;
use petgraph::prelude::DiGraphMap;
use std::collections::{HashMap, VecDeque};
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
impl From<ServerType> for GraphNodeType {
    fn from(value: ServerType) -> Self {
        match value {
            ServerType::ChatServer => GraphNodeType::Chat,
            ServerType::FileServer => GraphNodeType::Text,
            ServerType::MediaServer => GraphNodeType::Media,
        }
    }
}

impl GraphNodeType {
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

        Ok(ret)
    }

    fn defragment(v: &[Fragment], compr: Compression) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let mut msg: Vec<u8> = vec![];
        for f in v {
            msg.append(&mut f.data.to_vec().clone());
        }

        let decompressed = match compr {
            Compression::LZW => {
                let compressed = <Vec<u16>>::deserialize(msg)?;

                LZWCompressor::new().decompress(compressed).map_err(|e| {
                    println!("{e}");
                    SerializationError
                })?
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

        Ok(ret)
    }

    fn defragment(v: &[Fragment], compr: Compression) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let mut msg: Vec<u8> = vec![];
        for f in v {
            msg.append(&mut f.data.to_vec().clone());
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestType {
    TextList(NodeId),
    MediaList(NodeId),
    ServersType,
    Text(String, NodeId),
    Media(String, NodeId),
}

#[derive(Debug, Clone)]
struct WebBrowserRequest {
    request_id: RequestId,
    server_id: NodeId,
    waiting_for_ack: HashMap<PacketId, Fragment>, // stores the outgoing fragments that are still waiting for ACK
    incoming_messages: Vec<Fragment>, // stores the incoming fragments that compose the response of the query
    compression: Compression,
    request_type: RequestType,
    response_is_complete: bool,
}
impl WebBrowserRequest {
    fn new(
        request_id: RequestId,
        server_id: NodeId,
        waiting_for_ack: HashMap<PacketId, Fragment>,
        compression: Compression,
        request_type: RequestType,
    ) -> Self {
        Self {
            request_id,
            server_id,
            waiting_for_ack,
            incoming_messages: vec![],
            compression,
            request_type,
            response_is_complete: false,
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
    pending_requests: Vec<WebBrowserRequest>,
    packet_id_counter: PacketId,
    topology_graph: DiGraphMap<NodeId, u64>,
    nodes_type: HashMap<NodeId, GraphNodeType>,
    waiting_for_flood: VecDeque<(PacketId, Fragment)>, // stores the outgoing fragment for which I couldn't find a path or I received a NACK back instead of ACK
    text_media_map: HashMap<(NodeId, String), Vec<String>>, // links a text filename and the nodeId that provided it to the media filenames that it requires
    stored_files: HashMap<String, Vec<u8>>, // filename -> file
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
        let _ = self.controller_send
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
        for node in packet_send.keys() {
            initial_edges.push((id, *node));
            initial_edges.push((*node, id));
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
            waiting_for_flood: VecDeque::new(),
            text_media_map: HashMap::new(),
            stored_files: HashMap::new(),
        }
    }

    fn run(&mut self) {
        sleep(time::Duration::from_millis(100));
        self.start_flooding();

        loop {
            // complete request
            if let Some(i) = self
                .pending_requests
                .iter()
                .position(|req| req.response_is_complete && req.waiting_for_ack.is_empty())
            {
                let req = self.pending_requests.remove(i);
                self.complete_request(req);
            }

            // try sending a packet that received a NACK
            if let Some((id, frag)) = self.waiting_for_flood.pop_front() {
                match self
                    .pending_requests
                    .iter()
                    .find(|req| req.request_id == id.get_request_id())
                {
                    Some(req) => {
                        let p = Packet::new_fragment(
                            SourceRoutingHeader::empty_route(),
                            id.get_session_id(),
                            frag.clone(),
                        );

                        let (packet, channel) = self.prepare_packet_routing(p, req.server_id);
                        if let Some(channel) = channel {
                            self.send_packet(packet, channel);
                            continue;
                        } else {
                            self.waiting_for_flood
                                .push_back((PacketId::from_u64(packet.session_id), frag));
                        }
                    }

                    None => {
                        println!("client {} - Found a packet in waiting_for_flood without the corresponding request. BUG", self.id);
                        unreachable!()
                    }
                }
            }

            // check if there exist a text file that has all the media files available,
            // send response and clear the relative entries
            if let Some(key) = self
                .text_media_map
                .iter()
                .find(|(_, list)| {
                    for f in *list {
                        if self.stored_files.contains_key(f) {
                            return false;
                        }
                    }
                    true
                })
                .map(|(key, _)| key.clone())
            {
                self.send_text_and_media_back(key);
            }

            select_biased! {
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
    fn send_text_and_media_back(&mut self, key: (NodeId, String)) {
        if let Some(media_list) = self.text_media_map.remove(&key) {
            let mut file_list: Vec<Vec<u8>> = vec![];
            // ! unwrap should be safe at this point
            file_list.push(self.stored_files.remove(&key.1).unwrap());
            for media in media_list {
                file_list.push(self.stored_files.remove(&media).unwrap());
            }
            let _ = self.controller_send
            // ! self.id should be server_id that requested the text file
                .send(ClientEvent::FileFromClient(file_list, key.0));
        } else {
            println!("ddd");
        }
    }
    fn shortcut(&self, packet: Packet) {
        //packet.routing_header.decrease_hop_index();
        match packet.routing_header.destination() {
            Some(_) => {
                // ! refactor: function that prints error of this fails
                self.controller_send.send(ClientEvent::Shortcut(packet));
            }
            None => {
                println!("Client {} - Packet doesn't contain a destination, unable to shortcut, dropping", self.id);
            }
        }
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

    fn source_routing_header_from_graph_search(&self, dest: NodeId) -> Option<SourceRoutingHeader> {
        // TODO change edge cost when present in the graph
        astar(
            &self.topology_graph,
            self.id,
            |n| n == dest,
            |(_, _, weight)| *weight,
            |_| 0,
        )
        .map(|(_, path)| wg_2024::network::SourceRoutingHeader::with_first_hop(path))
    }

    fn is_correct_server_type(&self, server_id: NodeId, requested_type: GraphNodeType) -> bool {
        self.nodes_type
            .get(&server_id)
            .is_some_and(|t| *t == requested_type)
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

    fn check_routing_header(&self, header: &SourceRoutingHeader) -> Option<NackType> {
        if header
            .current_hop()
            .is_none_or(|next_id| next_id != self.id)
        {
            return Some(NackType::UnexpectedRecipient(self.id));
        }

        if header
            .next_hop()
            .is_none_or(|next_id| next_id != self.id && !self.packet_send.contains_key(&next_id))
        {
            return Some(NackType::ErrorInRouting(header.next_hop().unwrap()));
        }

        None
    }

    fn client_is_destination(&self, p: &Packet) -> bool {
        p.routing_header
            .destination()
            .is_some_and(|dest| dest == self.id)
    }

    fn find_request_index(&self, p: &Packet) -> Option<usize> {
        self.pending_requests
            .iter()
            .position(|req| req.request_id == PacketId::from_u64(p.session_id).get_request_id())
    }

    fn handle_flood_response(&mut self, mut packet: Packet, resp: &FloodResponse) {
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
                if let Some((from_id, from_type)) = prev {
                    if *id == self.id || from_id == self.id {
                        self.topology_graph.add_edge(from_id, *id, 1);
                        self.topology_graph.add_edge(*id, from_id, 1);
                    } else {
                        // this prevents A* to find path with client/server in the middle
                        if matches!(from_type, NodeType::Client)
                            | matches!(from_type, NodeType::Server)
                        {
                            self.topology_graph.add_edge(*id, from_id, 1);
                        } else if matches!(node_type, NodeType::Client)
                            | matches!(node_type, NodeType::Server)
                        {
                            self.topology_graph.add_edge(from_id, *id, 1);
                        } else {
                            self.topology_graph.add_edge(from_id, *id, 1);
                            self.topology_graph.add_edge(*id, from_id, 1);
                        }
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

    fn handle_ack(&mut self, packet: Packet, ack: &Ack) {
        if !self.client_is_destination(&packet) {
            self.shortcut(packet);
            return;
        }

        match self.find_request_index(&packet) {
            Some(id) => {
                let req = self.pending_requests.get_mut(id).unwrap();
                if req
                    .waiting_for_ack
                    .remove(&PacketId::from_u64(packet.session_id))
                    .is_none()
                {
                    println!("client {} - I received an ack {:?} for a packet that has already been acknowledged, bug for me or for the sender", self.id, ack);
                }
            }
            None => {
                println!(
                    "client {} - I received an ack for an unknown req_id, dropping",
                    self.id
                );
            }
        }
    }

    fn handle_fragment(&mut self, packet: Packet, fragment: Fragment) {
        if !self.client_is_destination(&packet) {
            self.shortcut(packet);
            return;
        }

        match self.find_request_index(&packet) {
            Some(id) => {
                let req = self.pending_requests.get_mut(id).unwrap();

                let n_frags = fragment.total_n_fragments as usize;

                // check if a fragment has been already received
                if req
                    .incoming_messages
                    .iter()
                    .any(|f| f.fragment_index == fragment.fragment_index)
                {
                    println!("client {} - I received the same fragment multiple times, bug, ignoring the message", self.id);
                    return;
                }

                req.incoming_messages.push(fragment.clone());

                if req.incoming_messages.len() == n_frags {
                    // I have all the fragments
                    req.response_is_complete = true;
                }

                let ack_dest = req.server_id;
                // send ACK to acknowledge the packet
                self.send_ack(ack_dest, packet.session_id, fragment.fragment_index);
            }
            None => {
                println!("client {} - I received a fragment for req_id \"{}\" that it's unknown to me, dropping", self.id, PacketId::from_u64(packet.session_id).get_request_id());
            }
        }
    }

    fn handle_packet(&mut self, packet: Packet) {
        println!("client {} - handling packet: {:?}", self.id, packet);
        match packet.pack_type.clone() {
            PacketType::FloodRequest(mut req) => {
                let _ =
                    self.handle_flood_request(&packet.routing_header, packet.session_id, &mut req);
            }

            PacketType::FloodResponse(ref resp) => {
                self.handle_flood_response(packet, resp);
            }

            PacketType::Ack(ref ack) => {
                self.handle_ack(packet, ack);
            }

            PacketType::Nack(nack) => {
                if !self.client_is_destination(&packet) {
                    self.shortcut(packet);
                    return;
                }

                match self.find_request_index(&packet) {
                    Some(id) => {
                        let req = self.pending_requests.get_mut(id).unwrap();

                        let fragment = match req
                            .waiting_for_ack
                            .get(&PacketId::from_u64(packet.session_id))
                        {
                            Some(f) => f.clone(),
                            None => {
                                println!("client {} - I received a NACK for packet_id \"{}\" that it's unknown to me, dropping", self.id, PacketId::from_u64(packet.session_id).get_packet_id());
                                return;
                            }
                        };

                        match nack.nack_type {
                            NackType::Dropped | NackType::DestinationIsDrone => {
                                // TODO update graph with some metric
                                let dest = req.server_id;
                                let p = Packet::new_fragment(
                                    SourceRoutingHeader::empty_route(),
                                    packet.session_id,
                                    fragment.clone(),
                                );

                                if let (packet, Some(channel)) =
                                    self.prepare_packet_routing(p, dest)
                                {
                                    self.send_packet(packet, channel);
                                } else {
                                    self.waiting_for_flood.push_back((
                                        PacketId::from_u64(packet.session_id),
                                        fragment.clone(),
                                    ));
                                    self.start_flooding();
                                }
                            }

                            NackType::ErrorInRouting(node_to_remove)
                            | NackType::UnexpectedRecipient(node_to_remove) => {
                                // remove problematic drone and search for a new path, if found send. otherwise start a flood
                                self.topology_graph.remove_node(node_to_remove);

                                // TODO update graph with some metric
                                let dest = req.server_id;
                                let p = Packet::new_fragment(
                                    SourceRoutingHeader::empty_route(),
                                    packet.session_id,
                                    fragment.clone(),
                                );
                                if let (packet, Some(channel)) =
                                    self.prepare_packet_routing(p, dest)
                                {
                                    self.send_packet(packet, channel);
                                } else {
                                    self.waiting_for_flood.push_back((
                                        PacketId::from_u64(packet.session_id),
                                        fragment.clone(),
                                    ));
                                    self.start_flooding();
                                }
                            }
                        }
                    }
                    None => {
                        println!("client {} - I received a NACK for req_id \"{}\" that it's unknown to me, dropping", self.id, PacketId::from_u64(packet.session_id).get_request_id());
                    }
                }
            }

            PacketType::MsgFragment(fragment) => {
                self.handle_fragment(packet, fragment);
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

    fn get_media_inside_text_file(&self, file_str: &str) -> Vec<String> {
        let document = scraper::Html::parse_document(file_str);
        let mut ret = vec![];
        if let Ok(selector) = scraper::Selector::parse("img") {
            for path in document.select(&selector) {
                if let Some(path) = path.value().attr("src") {
                    ret.push(path.to_string());
                }
            }
        }
        ret
    }

    fn complete_request(&mut self, mut req: WebBrowserRequest) {
        println!(
            "Client {}, completing req (from: {:?}, {:?}, to: {:?})",
            self.id, req.request_id, req.request_type, req.server_id
        );
        req.incoming_messages
            .sort_by(|f1, f2| f1.fragment_index.cmp(&f2.fragment_index));
        let content =
            web_messages::ResponseMessage::defragment(&req.incoming_messages, req.compression);

        if content.is_err() {
            println!("client {} - Cannot deserialize response, dropping", self.id);
            return;
        }

        match content.unwrap().content {
            Response::Generic(resp) => {
                match resp {
                    GenericResponse::Type(server_type) => {
                        self.nodes_type
                            .entry(req.server_id)
                            .and_modify(|t| *t = server_type.into());

                        // if I discovered all the server type
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
                        // ! add apposite clientevent
                        let _ = self.controller_send.send(ClientEvent::UnsupportedRequest);
                    }
                }
            }
            Response::Text(resp) => {
                match resp {
                    TextResponse::TextList(vec) => {
                        println!("sending message to scl {{{:?}}}", vec);
                        let _ = self
                            .controller_send
                            .send(ClientEvent::ListOfFiles(vec, req.server_id));
                    }
                    TextResponse::Text(file) => {
                        let file_str = String::from_utf8(file.clone()).unwrap();

                        let needed_media = self.get_media_inside_text_file(&file_str);

                        if !needed_media.is_empty() {
                            let text_filename = match req.request_type {
                                RequestType::Text(filename, _) => filename,
                                _ => {
                                    println!(
                                        "Response is not coherent with request, dropping request"
                                    );
                                    return;
                                }
                            };

                            // store the file while waiting for media
                            self.stored_files.insert(text_filename.clone(), file);

                            // store every media in stored_files as (filename, empty Vec)
                            needed_media.iter().for_each(|media_filename| {
                                self.stored_files.insert(media_filename.clone(), Vec::new());
                            });

                            // store media and text files links
                            self.text_media_map
                                .insert((req.server_id, text_filename.clone()), needed_media);

                            // create media list request
                            self.nodes_type
                                .iter()
                                .filter(|(_, t)| **t == GraphNodeType::Media)
                                .map(|(id, _)| *id)
                                .collect::<Vec<NodeId>>()
                                .iter()
                                .for_each(|id| {
                                    self.create_request(RequestType::MediaList(*id))
                                });
                        } else {
                            // TODO maybe send to scl a vec<string> if media are not embedded
                            let _ = self
                                .controller_send
                                .send(ClientEvent::FileFromClient(vec![file], req.server_id));
                        }
                    }
                }
            }
            Response::Media(resp) => {
                match resp {
                    MediaResponse::MediaList(file_list) => {
                        // check if medialist constains one of the media I need and ask for it
                        for filename in file_list {
                            if self.stored_files.contains_key(&filename) {
                                self.create_request(RequestType::Media(
                                    filename,
                                    req.server_id,
                                ));
                            }
                        }
                    }
                    MediaResponse::Media(file) => {
                        // put the media together with its text file and, if no more media are needed, send to scl
                        let media_filename = match req.request_type {
                            RequestType::Media(filename, _) => filename,
                            _ => {
                                println!("Response is not coherent with request, dropping request");
                                return;
                            }
                        };

                        self.stored_files
                            .entry(media_filename.clone())
                            .and_modify(|content| *content = file);
                    }
                }
            }
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
                self.create_request(RequestType::TextList(server_id));
            }

            ClientCommand::AskServersTypes => {
                self.create_request(RequestType::ServersType);
            }

            ClientCommand::RequestFile(filename, server_id) => {
                self.create_request(RequestType::Text(filename, server_id))
            }

            ClientCommand::Shortcut(packet) => self.handle_packet(packet),

            _ => println!(
                "client {} - Chat messages are not implemented here!",
                self.id
            ),
        }
    }

    fn start_flooding(&mut self) {
        println!("client {} - starting flood", self.id);

        let p = Packet::new_flood_request(
            SourceRoutingHeader::empty_route(),
            0, // sessionId is useless in flood requests and responses
            FloodRequest::initialize(self.sequential_flood_id, self.id, NodeType::Client),
        );

        self.flood_history.entry(self.id).and_modify(|ring| {
            ring.insert(self.sequential_flood_id);
        });

        for channel in self.packet_send.values() {
            self.send_packet(p.clone(), channel);
        }

        self.sequential_flood_id += 1;
    }

    fn add_request(
        &mut self,
        server_id: NodeId,
        compression: Compression,
        frags: Vec<Fragment>,
        request_type: RequestType,
    ) {
        println!("Adding request");

        let mut new_request = WebBrowserRequest::new(
            self.packet_id_counter.get_request_id(),
            server_id,
            HashMap::new(),
            compression,
            request_type,
        );

        for f in frags {
            let p = wg_2024::packet::Packet::new_fragment(
                SourceRoutingHeader::empty_route(),
                self.packet_id_counter.get_session_id(),
                f.clone(),
            );
            new_request
                .waiting_for_ack
                .insert(self.packet_id_counter.clone(), f.clone());

            if let (packet, Some(channel)) = self.prepare_packet_routing(p, server_id) {
                self.send_packet(packet, channel);
            } else {
                self.waiting_for_flood
                    .push_back((self.packet_id_counter.clone(), f));
                self.start_flooding();
            }

            self.packet_id_counter.increment_packet_id();
        }

        self.pending_requests.push(new_request);

        self.packet_id_counter.increment_request_id();
    }

    fn create_request(&mut self, request_type: RequestType) {
        let compression: Compression; // TODO has to be chosen by scl or randomically

        match &request_type {
            RequestType::TextList(server_id) => {
                compression = Compression::None;
                let frags =
                    web_messages::RequestMessage::new_text_list_request(self.id, compression.clone())
                        .fragment()
                        .expect("Error during fragmentation. This can't happen. If it happens there is a bug somwhere");

                self.add_request(*server_id, compression, frags, request_type);
            }

            RequestType::MediaList(server_id) => {
                compression = Compression::None;
                let frags =
                    web_messages::RequestMessage::new_media_list_request(self.id, compression.clone())
                        .fragment()
                        .expect("Error during fragmentation. This can't happen. If it happens there is a bug somwhere");

                self.add_request(*server_id, compression, frags, request_type);
            }

            RequestType::ServersType => {
                compression = Compression::None;
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

            RequestType::Media(filename, server_id) => {
                compression = Compression::None;
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

            RequestType::Text(filename, server_id) => {
                compression = Compression::LZW;
                if !self.is_correct_server_type(*server_id, GraphNodeType::Text) {
                    println!(
                        "client {} - cannot ask for file because it is not a text file",
                        self.id
                    );
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
