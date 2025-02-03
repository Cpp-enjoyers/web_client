#![warn(clippy::pedantic)]

use common::slc_commands::{ServerType, TextMediaResponse, WebClientCommand, WebClientEvent};
use compression::huffman::HuffmanCompressor;
use core::time;
use crossbeam_channel::{select_biased, Receiver, SendError, Sender};
use petgraph::algo::astar;
use petgraph::prelude::DiGraphMap;
use std::collections::{HashMap, VecDeque};
use std::thread::sleep;
use std::vec;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{
    Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType, FRAGMENT_DSIZE
};

use common::networking::flooder::Flooder;
use common::ring_buffer::RingBuffer;
use common::web_messages;
use common::web_messages::{
    Compression, GenericResponse, MediaResponse, RequestMessage, Response, ResponseMessage,
    Serializable, SerializableSerde, SerializationError, TextResponse,
};
use common::Client;
use compression::lzw::LZWCompressor;
use compression::Compressor;

mod packet_id;
use packet_id::{PacketId, RequestId};

mod web_client_test;

const RING_BUFF_SZ: usize = 64;
const DEFAULT_PDR: f64 = 0.5;


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
                let compressed = <Vec<u16> as Serializable>::deserialize(msg)?;

                LZWCompressor::new().decompress(compressed).map_err(|e| {
                    println!("{e}");
                    SerializationError
                })?
            }

            Compression::None => msg,

            Compression::Huffman => {
                let compressed = <<HuffmanCompressor as Compressor>::Compressed>::deserialize(msg)?;

                HuffmanCompressor::new().decompress(compressed).map_err(|e| {
                    println!("{e}");
                    SerializationError
                })?
            }
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
                let compressed = <Vec<u16> as Serializable>::deserialize(msg)?;
                LZWCompressor::new()
                    .decompress(compressed)
                    .map_err(|_| SerializationError)?
            }

            Compression::None => msg,

            Compression::Huffman => {
                let compressed = <<HuffmanCompressor as Compressor>::Compressed>::deserialize(msg)?;

                HuffmanCompressor::new().decompress(compressed).map_err(|e| {
                    println!("{e}");
                    SerializationError
                })?
            }
        };

        Self::deserialize(decompressed)
    }
}


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
    controller_send: Sender<WebClientEvent>,
    controller_recv: Receiver<WebClientCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    flood_history: HashMap<NodeId, RingBuffer<u64>>,
    sequential_flood_id: u64,
    pending_requests: Vec<WebBrowserRequest>,
    packet_id_counter: PacketId,
    topology_graph: DiGraphMap<NodeId, f64>,
    nodes_type: HashMap<NodeId, GraphNodeType>,
    packets_to_bo_sent_again: VecDeque<(PacketId, Fragment)>, // stores the outgoing fragment for which I couldn't find a path or I received a NACK back instead of ACK
    text_media_map: HashMap<(NodeId, String), Vec<String>>, // links a text filename and the nodeId that provided it to the media filenames that it requires
    stored_files: HashMap<String, Vec<u8>>,                 // filename -> file
    //media_file_either_owner_or_counter: HashMap<String, Either<Option<NodeId>, u8>>, // for every media file store either the owner or the n. of remaining media list responses to know the owner
    media_owner: HashMap<String, Option<NodeId>>, // media server which owns the media, if any
    media_request_left: HashMap<String, u8>, // how many media list response I have to wait before knowing that media is unavailable
    packets_sent_counter: HashMap<NodeId, (u64, u64)>, // track the number of packets (sent, lost) through every drone
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
        if let Some(set) = self.flood_history.get_mut(&flood_id.0) {
            set.insert(flood_id.1);
        } else {
            let mut rb = RingBuffer::with_capacity(RING_BUFF_SZ);
            rb.insert(flood_id.1);
            self.flood_history.insert(flood_id.0, rb);
        }
    }

    fn send_to_controller(&self, p: Packet) {
        self.internal_send_to_controller(WebClientEvent::Shortcut(p));
    }
}

impl Client<WebClientCommand, WebClientEvent> for WebBrowser {
    fn new(
        id: NodeId,
        controller_send: Sender<WebClientEvent>,
        controller_recv: Receiver<WebClientCommand>,
        packet_recv: Receiver<Packet>,
        packet_send: HashMap<NodeId, Sender<Packet>>,
    ) -> Self {
        let mut initial_edges = vec![];
        for node in packet_send.keys() {
            initial_edges.push((id, *node, DEFAULT_PDR));
            initial_edges.push((*node, id, DEFAULT_PDR));
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
            packets_to_bo_sent_again: VecDeque::new(),
            text_media_map: HashMap::new(),
            stored_files: HashMap::new(),
            media_request_left: HashMap::new(),
            media_owner: HashMap::new(),
            packets_sent_counter: HashMap::new()
        }
    }

    fn run(&mut self) {
        sleep(time::Duration::from_millis(100));
        self.start_flooding();

        loop {
            // complete request
            self.try_complete_request();

            // try sending a packet that received a NACK
            self.try_resend_packet();

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

    // ! create file for utils, separate tests in more files, unit vs integration tests,

    // TESTED
    fn get_filename_from_path(s: &String) -> String{
        s.split('/').last().unwrap_or(s).to_string()
    }

    // TESTED
    fn internal_send_to_controller(&self, msg: WebClientEvent) {
        if let Err(e) = self.controller_send.send(msg) {
            println!("client {} - cannot send to scl: {e:?}", self.id);
        }
    }

    // TESTED
    fn try_resend_packet(&mut self) {
        if let Some((id, frag)) = self.packets_to_bo_sent_again.pop_front() {
            if let Some(req) = self
                .pending_requests
                .iter()
                .find(|req| req.request_id == id.get_request_id())
            {
                let packet = Packet::new_fragment(
                    SourceRoutingHeader::empty_route(),
                    id.get_session_id(),
                    frag.clone(),
                );

                if let Err(e) = self.try_send_packet(packet.clone(), req.server_id){
                    self.packets_to_bo_sent_again
                        .push_back((PacketId::from_u64(e.0.session_id), frag));
                }
            } else {
                println!("client {} - Found a packet in waiting_for_flood without the corresponding request. BUG", self.id);
                unreachable!()
            }
        }
    }

    fn try_complete_request(&mut self) {
        if let Some(i) = self
            .pending_requests
            .iter()
            .position(|req| req.response_is_complete && req.waiting_for_ack.is_empty())
        {
            let req = self.pending_requests.remove(i);
            self.complete_request(req);
        }

        // todo
        // search for an entry that, for each of the needed media, it either is in cache or the owner is None
        if let Some(key) = self
            .text_media_map
            .iter()
            .find(|(_, list)| {
                for f in *list {
                    if (self
                        .media_owner
                        .get(f)
                        .is_some_and(std::option::Option::is_none)
                        && self.media_request_left.get(f).is_some_and(|r| *r > 0)
                        && !self.stored_files.contains_key(f))
                        || (!self.stored_files.contains_key(f)
                            && self
                                .media_owner
                                .get(f)
                                .is_some_and(std::option::Option::is_some))
                    {
                        return false;
                    }
                }
                true
            })
            .map(|(key, _)| key.clone())
        {
            self.send_text_and_media_back(&key);
        }
    }

    fn send_text_and_media_back(&mut self, key: &(NodeId, String)) {
        println!("send_text_and_media_back");
        if let Some(media_list) = self.text_media_map.remove(key) {
            // ! unwrap of the text file must work
            let html_file = (
                Self::get_filename_from_path(&key.1),
                self.stored_files.remove(&key.1).unwrap(),
            );
            let mut media_files = vec![];

            for media_full_name in media_list {
                media_files.push((
                    Self::get_filename_from_path(&media_full_name),
                    self.stored_files
                        .remove(&media_full_name)
                        .unwrap_or_default(),
                ));
                self.media_owner.remove(&media_full_name);
                self.media_request_left.remove(&media_full_name);
            }

            self.internal_send_to_controller(WebClientEvent::FileFromClient(
                TextMediaResponse::new(html_file, media_files),
                key.0,
            ));
        }
    }

    fn shortcut(&self, packet: Packet) {
        //packet.routing_header.decrease_hop_index();
        match packet.routing_header.destination() {
            Some(_) => {
                self.internal_send_to_controller(WebClientEvent::Shortcut(packet));
            }
            None => {
                println!("Client {} - Packet doesn't contain a destination, unable to shortcut, dropping", self.id);
            }
        }
    }

    fn try_send_packet(&mut self, mut p: Packet, dest: NodeId) -> Result<(), SendError<Packet>>{
        let mut final_packet: Packet = p.clone();
        match p.pack_type{
            PacketType::MsgFragment(_) => {
                let (packet, channel) = self.prepare_packet_routing(p, dest);
                if let Some(channel) = channel {
                    if let Err(e) = channel.send(packet.clone()) {
                        println!("client {} - CANNOT send packet {:?} : {e}", self.id, e.0);
                        return Err(e);
                    }
                    let _ = self
                        .controller_send
                        .send(WebClientEvent::PacketSent(packet.clone()));

                    if let PacketType::MsgFragment(_) = &packet.pack_type{
                        for drone_id in &packet.routing_header.hops[1..]{
                            self.increment_packet_sent_counter(*drone_id);
                        }
                    }
                    final_packet = packet;
                }
                else {
                    return Err(SendError(packet));
                }

            }

            PacketType::FloodResponse(_) => {
                if let Some(channel) = self.packet_send.get(&dest) {
                    p.routing_header.increase_hop_index();
                    if let Err(e) = channel.send(p.clone()) {
                        println!("client {} - CANNOT send packet {:?} : {e}", self.id, e.0);
                        return Err(e);
                    }
                    final_packet = p;
                }
            }

            PacketType::Nack(_) => {
                if let (packet, Some(channel)) = self.prepare_packet_routing(p, dest) {
                    if let Err(e) = channel.send(packet.clone()) {
                        println!("client {} - CANNOT send packet {:?} : {e}", self.id, e.0);
                        return Err(e);
                    }
                    final_packet = packet;
                }

            }

            PacketType::Ack(_) => {
                let (mut packet, opt_chn) = self.prepare_packet_routing(p, dest);
                if let Some(channel) = opt_chn {
                    if let Err(e) = channel.send(packet.clone()) {
                        println!("client {} - CANNOT send packet {:?} : {e}", self.id, e.0);
                        return Err(e);
                    }
                    final_packet = packet
                }
            }

            _ => unreachable!()

        }
        self.controller_send.send(WebClientEvent::PacketSent(final_packet));
        Ok(())
    }

    fn source_routing_header_from_graph_search(&self, dest: NodeId) -> Option<SourceRoutingHeader> {
        astar(
            &self.topology_graph,
            self.id,
            |n| n == dest,
            |(_, _, weight)| *weight,
            |_| 0.,
        )
        .map(|(_, path)| wg_2024::network::SourceRoutingHeader::with_first_hop(path))
    }

    fn is_correct_server_type(&self, server_id: NodeId, requested_type: &GraphNodeType) -> bool {
        self.nodes_type
            .get(&server_id)
            .is_some_and(|t| t == requested_type)
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
                    println!("client {} - found a path towards {:?}", self.id, dest);

                    return (packet, Some(channel));
                }
            }
        }
        println!("client {} - CANNOT find a path towards {}", self.id, dest);

        (packet, None)
    }

    fn client_is_destination(&self, p: &Packet) -> bool {
        p.routing_header
            .destination()
            .is_some_and(|dest| dest == self.id)
    }

    fn get_request_index(&self, p: &Packet) -> Option<usize> {
        self.pending_requests
            .iter()
            .position(|req| req.request_id == PacketId::from_u64(p.session_id).get_request_id())
    }

    fn add_edge_if_not_already_in(&mut self, from: NodeId, to: NodeId, weight: f64){
        if !self.topology_graph.contains_edge(from, to){
            self.topology_graph.add_edge(from, to, weight);
        }
    }

    fn handle_flood_response(&mut self, packet: Packet, resp: &FloodResponse) {
        let initiator: Option<&(NodeId, NodeType)> = resp.path_trace.first();

        if initiator.is_none() {
            println!(
                "client {} - Received a flood response with empty path trace, dropping",
                self.id
            );
            return;
        }

        if initiator.unwrap().0 == self.id{
            if resp.flood_id < self.sequential_flood_id{
                println!("client {} -received an old flood response, dropping", self.id);
                return;
            }
            let mut prev: Option<(NodeId, NodeType)> = None;
            for (id, node_type) in &resp.path_trace {
                if let Some((from_id, from_type)) = prev {
                    if *id == self.id || from_id == self.id {
                        self.add_edge_if_not_already_in(from_id, *id, DEFAULT_PDR);
                        self.add_edge_if_not_already_in(*id, from_id, DEFAULT_PDR);
                    } else {
                        // this prevents A* to find path with client/server in the middle
                        if matches!(from_type, NodeType::Client)
                            | matches!(from_type, NodeType::Server)
                        {
                            self.add_edge_if_not_already_in(*id, from_id, DEFAULT_PDR);
                        } else if matches!(node_type, NodeType::Client)
                            | matches!(node_type, NodeType::Server)
                        {
                            self.add_edge_if_not_already_in(from_id, *id, DEFAULT_PDR);
                        } else {
                            self.add_edge_if_not_already_in(from_id, *id, DEFAULT_PDR);
                            self.add_edge_if_not_already_in(*id, from_id, DEFAULT_PDR);
                        }
                    }

                    self.nodes_type
                        .insert(*id, GraphNodeType::from_node_type(*node_type));

                    // initialize the packet counter
                    for (drone_id, _) in self.nodes_type.iter().filter(|(_, t)| **t == GraphNodeType::Drone){
                        self.packets_sent_counter.entry(*drone_id).insert_entry((0,0));
                    }
                }
                prev = Some((*id, *node_type));
            }

            println!(
                "client: {} - graph: {:?} - nodes type: {:?}",
                self.id, self.topology_graph, self.nodes_type
            );
        } else if let Some(next_hop_drone_id) = packet.routing_header.next_hop() {
            if let Err(e) = self.try_send_packet(packet, next_hop_drone_id){
                // I don't have the channel to forward the packet - SHORTCUT
                self.shortcut(e.0);
            }
        } else {
            println!("client {} - Found a flood response with a corrupted routing header, I don't know who is the next hop nor, consequently, the original initiator to shortcut this packet. Dropping", self.id);
            unreachable!()
        }
    }

    fn handle_ack(&mut self, packet: Packet, ack: &Ack) {
        if !self.client_is_destination(&packet) {
            self.shortcut(packet);
            return;
        }

        match self.get_request_index(&packet) {
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

    fn handle_fragment(&mut self, packet: Packet, fragment: &Fragment) {
        if !self.client_is_destination(&packet) {
            self.shortcut(packet);
            return;
        }

        match self.get_request_index(&packet) {
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
                let ack = Packet::new_ack(
                    SourceRoutingHeader::empty_route(),
                    packet.session_id,
                    fragment.fragment_index,
                );

                if let Err(mut e) = self.try_send_packet(ack, ack_dest) {
                    println!(
                        "client {} - Can't find a path to the node, I need to shortcut ACK",
                        self.id
                    );
                    e.0.routing_header = SourceRoutingHeader::initialize(vec![self.id, ack_dest]);
                    self.shortcut(e.0);
                }
            }
            None => {
                println!("client {} - I received a fragment for req_id \"{}\" that it's unknown to me, dropping", self.id, PacketId::from_u64(packet.session_id).get_request_id());
            }
        }
    }

    fn increment_packet_lost_counter(&mut self, drone_id: NodeId){
        self.packets_sent_counter.entry(drone_id).and_modify(|(sent, c)| if *c < *sent {*c += 1} );

        let pdr = self.packets_sent_counter.get(&drone_id).map_or(1., |(d, n)| *n as f64 / *d as f64);

        for (_, _, weight) in self.topology_graph.all_edges_mut().filter(|(_, to, _)| *to == drone_id){
            *weight = pdr;
        }
    }

    fn increment_packet_sent_counter(&mut self, drone_id: NodeId){
        self.packets_sent_counter.entry(drone_id).and_modify(|(c, _)| *c += 1);

        let pdr = self.packets_sent_counter.get(&drone_id).map_or(1., |(d, n)| *n as f64 / *d as f64);

        for (_, _, weight) in self.topology_graph.all_edges_mut().filter(|(_, to, _)| *to == drone_id){
            *weight = pdr;
        }
    }

    fn handle_nack(&mut self, packet: Packet, nack: &Nack) {
        if !self.client_is_destination(&packet) {
            self.shortcut(packet);
            return;
        }

        match self.get_request_index(&packet) {
            Some(id) => {
                let req = self.pending_requests.get_mut(id).unwrap();

                let fragment = if let Some(f) = req
                    .waiting_for_ack
                    .get(&PacketId::from_u64(packet.session_id))
                {
                    f.clone()
                } else {
                    println!("client {} - I received a NACK for packet_id \"{}\" that it's unknown to me, dropping", self.id, PacketId::from_u64(packet.session_id).get_packet_id());
                    return;
                };

                match nack.nack_type {
                    NackType::Dropped | NackType::DestinationIsDrone => {

                        let dest = req.server_id;
                        let new_packet = Packet::new_fragment(
                            SourceRoutingHeader::empty_route(),
                            packet.session_id,
                            fragment.clone(),
                        );

                        if let Err(_) = self.try_send_packet(new_packet.clone(), dest){
                            self.packets_to_bo_sent_again.push_back((
                                PacketId::from_u64(packet.session_id),
                                fragment.clone(),
                            ));
                            self.start_flooding();
                        }

                        // update edges weight
                        if let NackType::Dropped = nack.nack_type {
                            if let Some(drone_id) = packet.routing_header.source(){
                                self.increment_packet_lost_counter(drone_id);
                            }
                        }
                    }

                    NackType::ErrorInRouting(node_to_remove)
                    | NackType::UnexpectedRecipient(node_to_remove) => {
                        // remove problematic drone and search for a new path, if found send. otherwise start a flood
                        self.topology_graph.remove_node(node_to_remove);

                        let dest = req.server_id;
                        let p = Packet::new_fragment(
                            SourceRoutingHeader::empty_route(),
                            packet.session_id,
                            fragment.clone(),
                        );
                        if let Err(_) = self.try_send_packet(p, dest) {
                            self.packets_to_bo_sent_again.push_back((
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
                self.handle_nack(packet, &nack);
            }

            PacketType::MsgFragment(fragment) => {
                self.handle_fragment(packet, &fragment);
            }
        }
    }

    fn get_media_inside_text_file(file_str: &str) -> Vec<String> {
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

    fn complete_request_with_generic_response(
        &mut self,
        server_id: NodeId,
        request_type: RequestType,
        resp: GenericResponse,
    ) {
        match resp {
            GenericResponse::Type(server_type) => {
                self.nodes_type
                    .entry(server_id)
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
                    self.internal_send_to_controller(WebClientEvent::ServersTypes(list));
                }
            }
            GenericResponse::InvalidRequest => {
                self.internal_send_to_controller(WebClientEvent::UnsupportedRequest);
            }
            GenericResponse::NotFound => {
                // ! add apposite clientevent
                self.internal_send_to_controller(WebClientEvent::UnsupportedRequest);
            }
        }
    }

    fn complete_request_with_text_response(
        &mut self,
        server_id: NodeId,
        request_type: RequestType,
        resp: TextResponse,
    ) {
        match resp {
            TextResponse::TextList(vec) => {
                println!("sending message to scl {{{:?}}}", vec);
                self.internal_send_to_controller(WebClientEvent::ListOfFiles(vec, server_id));
            }
            TextResponse::Text(file) => {
                let RequestType::Text(text_path, _) = request_type else {
                    println!("Response is not coherent with request, dropping request");
                    return;
                };

                let file_str = String::from_utf8(file.clone()).unwrap();

                let needed_media = Self::get_media_inside_text_file(&file_str);

                if needed_media.is_empty() {
                    self.internal_send_to_controller(WebClientEvent::FileFromClient(
                        TextMediaResponse::new((Self::get_filename_from_path(&text_path), file), Vec::new()),
                        server_id,
                    ));
                } else {
                    println!(
                        "client {} - I need {:?} for this file",
                        self.id, needed_media
                    );

                    // store the file while waiting for media
                    self.stored_files.insert(text_path.clone(), file);

                    // store media and text files links
                    self.text_media_map
                        .insert((server_id, text_path.clone()), needed_media.clone());

                    let mut is_required_media_list_request = false;
                    // store every media in stored_files as (filename, empty Vec)

                    // TODO
                    // for every media:
                    // if present in cache do nothing
                    // else, if I know the owner, do nothing (the file is arriving)
                    // else, if remaining list counter is set, do nothing(it's already been asked)
                    // else, set counter and ask media lists

                    for media_path in needed_media {
                        // ! refactor

                        if self
                            .media_owner
                            .get(&media_path)
                            .is_none_or(std::option::Option::is_none)
                        {
                            //self.stored_files.insert(media_filename.clone(), Vec::new());
                            self.media_owner.insert(media_path.clone(), None);
                            self.media_request_left.insert(
                                media_path.clone(),
                                self.nodes_type
                                    .iter()
                                    .filter(|(_, t)| **t == GraphNodeType::Media)
                                    .count() as u8,
                            );
                            println!(
                                "client {} - number of media client: {}",
                                self.id,
                                self.nodes_type
                                    .iter()
                                    .filter(|(_, t)| **t == GraphNodeType::Media)
                                    .count() as u8
                            );
                            is_required_media_list_request = true;
                        } else {
                            self.create_request(RequestType::Media(
                                media_path.clone(),
                                self.media_owner.get(&media_path).unwrap().unwrap(),
                            ));
                        }

                        println!("client {} - {:?}", self.id, self.media_owner);
                        println!("client {} - {:?}", self.id, self.media_request_left);
                    }

                    if is_required_media_list_request {
                        // create media list request
                        self.nodes_type
                            .iter()
                            .filter(|(_, t)| **t == GraphNodeType::Media)
                            .map(|(id, _)| *id)
                            .collect::<Vec<NodeId>>()
                            .iter()
                            .for_each(|id| {
                                println!("client {} - creating media list request", self.id);
                                self.create_request(RequestType::MediaList(*id));
                            });
                    }
                }
            }
        }
    }

    fn complete_request_with_media_response(
        &mut self,
        server_id: NodeId,
        request_type: RequestType,
        resp: MediaResponse,
    ) {
        match resp {
            MediaResponse::MediaList(file_list) => {
                // TODO
                // for every needed file, -1 to the counter
                // for every file in the list *that is needed*, if owner not set, set it and create request
                // else, do nothing (the file is arriving)
                for counter in &mut self.media_request_left.values_mut() {
                    *counter = counter.saturating_sub(1);
                }

                for media_path in file_list {
                    if self.text_media_map.values().any(|v| v.contains(&media_path)) {
                        self.media_owner.insert(media_path.clone(), Some(server_id));
                        self.media_request_left.insert(media_path.clone(), 0);
                        self.create_request(RequestType::Media(media_path, server_id));
                    }
                }
            }
            MediaResponse::Media(file) => {
                let RequestType::Media(media_path, _) = request_type else {
                    println!("Response is not coherent with request, dropping request");
                    return;
                };
                // todo
                // store in the cache

                self.stored_files.insert(media_path.clone(), file);
            }
        }
    }

    fn complete_request(&mut self, mut req: WebBrowserRequest) {
        // ! maybe add a check for each subfunction to check if the reqesut and response types are coeherent
        println!(
            "Client {}, completing req (id: {:?}, {:?}, to: {:?})",
            self.id, req.request_id, req.request_type, req.server_id
        );
        req.incoming_messages
            .sort_by(|f1, f2| f1.fragment_index.cmp(&f2.fragment_index));

        if let Ok(response_msg) =
            web_messages::ResponseMessage::defragment(&req.incoming_messages, req.compression)
        {
            match response_msg.content {
                Response::Generic(resp) => {
                    self.complete_request_with_generic_response(
                        req.server_id,
                        req.request_type,
                        resp,
                    );
                }
                Response::Text(resp) => {
                    self.complete_request_with_text_response(req.server_id, req.request_type, resp);
                }
                Response::Media(resp) => {
                    self.complete_request_with_media_response(
                        req.server_id,
                        req.request_type,
                        resp,
                    );
                }
            }
        } else {
            println!("client {} - Cannot deserialize response, dropping", self.id);
        }
    }

    fn handle_command(&mut self, command: WebClientCommand) {
        println!("Handling command {:?}", command);
        match command {
            WebClientCommand::AddSender(id, sender) => {
                self.packet_send.insert(id, sender);
                self.add_edge_if_not_already_in(self.id, id, DEFAULT_PDR);
                self.add_edge_if_not_already_in(id, self.id, DEFAULT_PDR);
                self.start_flooding();
            }

            WebClientCommand::RemoveSender(id) => {
                self.packet_send.remove(&id);
                self.topology_graph.remove_node(id);
            }

            WebClientCommand::AskListOfFiles(server_id) => {
                self.create_request(RequestType::TextList(server_id));
            }

            WebClientCommand::AskServersTypes => {
                self.create_request(RequestType::ServersType);
            }

            WebClientCommand::RequestFile(filename, server_id) => {
                self.create_request(RequestType::Text(filename, server_id));
            }

            WebClientCommand::Shortcut(packet) => self.handle_packet(packet),
        }
    }

    fn start_flooding(&mut self) {
        self.sequential_flood_id += 1;
        println!("client {} - starting flood", self.id);

        let packet = Packet::new_flood_request(
            SourceRoutingHeader::empty_route(),
            0, // sessionId is useless in flood requests and responses
            FloodRequest::initialize(self.sequential_flood_id, self.id, NodeType::Client),
        );

        self.flood_history.entry(self.id).and_modify(|ring| {
            ring.insert(self.sequential_flood_id);
        });

        for channel in self.packet_send.values() {
            println!("dc");
            channel.send(packet.clone());
            self.controller_send.send(WebClientEvent::PacketSent(packet.clone()));
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

            if let Err(_) = self.try_send_packet(p, server_id) {
                self.packets_to_bo_sent_again
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

            RequestType::Media(file_path, server_id) => {
                compression = Compression::None;
                if !self.is_correct_server_type(*server_id, &GraphNodeType::Media) {
                    self.internal_send_to_controller(WebClientEvent::UnsupportedRequest);
                    return;
                }

                let frags =
                    web_messages::RequestMessage::new_media_request(self.id, compression.clone(), file_path.clone())
                        .fragment()
                        .expect("Error during fragmentation. This can't happen. If it happens there is a bug somewhere");

                self.add_request(*server_id, compression, frags, request_type);
            }

            RequestType::Text(file_path, server_id) => {
                compression = Compression::LZW;
                if !self.is_correct_server_type(*server_id, &GraphNodeType::Text) {
                    println!(
                        "client {} - cannot ask for file because it is not a text file",
                        self.id
                    );
                    self.internal_send_to_controller(WebClientEvent::UnsupportedRequest);
                    return;
                }

                let frags =
                    web_messages::RequestMessage::new_text_request(self.id, compression.clone(), file_path.clone())
                        .fragment()
                        .expect("Error during fragmentation. This can't happen. If it happens there is a bug somwhere");

                self.add_request(*server_id, compression, frags, request_type);
            }
        }
    }
}
