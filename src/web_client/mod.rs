use common::slc_commands::{ServerType, TextMediaResponse, WebClientCommand, WebClientEvent};
use compression::huffman::HuffmanCompressor;
use flooding::RING_BUFF_SZ;
use core::time;
use crossbeam_channel::{select_biased, Receiver, SendError, Sender};
use itertools::Either;
use petgraph::algo::astar;
use petgraph::prelude::DiGraphMap;
use std::collections::{HashMap, VecDeque};
use std::thread::sleep;
use std::vec;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{
    Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType,
    FRAGMENT_DSIZE,
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

use crate::utils::{get_filename_from_path, get_media_inside_html_file, PacketId, RequestId};

mod flooding;

#[cfg(test)]
mod test;
#[cfg(test)]
mod utils_for_test;

const DEFAULT_WEIGHT: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq)]
enum GraphNodeType {
    TextServer,
    MediaServer,
    ChatServer,
    Server,
    Client,
    Drone,
}
impl From<ServerType> for GraphNodeType {
    fn from(value: ServerType) -> Self {
        match value {
            ServerType::ChatServer => GraphNodeType::ChatServer,
            ServerType::FileServer => GraphNodeType::TextServer,
            ServerType::MediaServer => GraphNodeType::MediaServer,
        }
    }
}
impl From<NodeType> for GraphNodeType {
    fn from(value: NodeType) -> Self {
        match value {
            NodeType::Client => GraphNodeType::Client,
            NodeType::Drone => GraphNodeType::Drone,
            NodeType::Server => GraphNodeType::Server,
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

                HuffmanCompressor::new()
                    .decompress(compressed)
                    .map_err(|e| {
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

                HuffmanCompressor::new()
                    .decompress(compressed)
                    .map_err(|e| {
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

#[derive(Debug, Clone, PartialEq)]
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
    media_file_either_owner_or_request_left: HashMap<String, Either<Option<NodeId>, Vec<NodeId>>>, // for every media file store either the owner or the n. of remaining media list responses to know the owner
    packets_sent_counter: HashMap<NodeId, (f64, f64)>, // track the number of packets (sent, lost) through every drone
    routing_header_history: HashMap<PacketId, SourceRoutingHeader>, // keep track of the route of each packet until ack or nack are received, in order to correctly estimate PDR
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
        let mut packets_sent_counter = HashMap::new();
        for node in packet_send.keys() {
            initial_edges.push((id, *node, DEFAULT_WEIGHT));
            initial_edges.push((*node, id, DEFAULT_WEIGHT));

            packets_sent_counter.insert(*node, (0., 0.));
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
            packets_sent_counter,
            media_file_either_owner_or_request_left: HashMap::new(),
            routing_header_history: HashMap::new(),
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
    // ! unit vs integration tests, logs

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

                match self.try_send_packet(packet, req.server_id) {
                    Ok(packet_sent) => {
                        self.routing_header_history.insert(
                            PacketId::from_u64(packet_sent.session_id),
                            packet_sent.routing_header.clone(),
                        );
                    }

                    Err(e) => {
                        self.packets_to_bo_sent_again
                            .push_back((PacketId::from_u64(e.0.session_id), frag));
                    }
                }
            } else {
                println!("client {} - Found a packet in waiting_for_flood without the corresponding request. BUG", self.id);
                unreachable!()
            }
        }
    }

    // TESTED
    fn try_complete_request(&mut self) {
        if let Some(i) = self
            .pending_requests
            .iter()
            .position(|req| req.response_is_complete && req.waiting_for_ack.is_empty())
        {
            let req = self.pending_requests.remove(i);
            self.complete_request(req);
        }

        // search for an entry that, for each of the needed media, it either is in cache or the owner is None

        if let Some(key) = self
            .text_media_map
            .iter()
            .find(|(_, list)| {
                for f in *list {
                    let owner_can_still_be_discovered = self
                        .media_file_either_owner_or_request_left
                        .get(f)
                        .is_some_and(|either| {
                            if let itertools::Either::Right(requests_left) = either {
                                return !requests_left.is_empty();
                            }
                            false
                        });

                    let media_is_already_stored = self.stored_files.contains_key(f);

                    let owner_is_in_the_graph = self
                        .media_file_either_owner_or_request_left
                        .get(f)
                        .is_some_and(|either| {
                            if let itertools::Either::Left(owner) = either {
                                return owner.is_some();
                            }
                            false
                        });

                    if !media_is_already_stored
                        && (owner_can_still_be_discovered || owner_is_in_the_graph)
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

    // TESTED
    fn send_text_and_media_back(&mut self, key: &(NodeId, String)) {
        println!("send_text_and_media_back");
        if let Some(media_list) = self.text_media_map.remove(key) {
            // ! unwrap of the text file must work
            let html_file = (
                get_filename_from_path(&key.1),
                self.stored_files.remove(&key.1).unwrap(),
            );
            let mut media_files = vec![];

            for media_full_name in media_list {
                media_files.push((
                    get_filename_from_path(&media_full_name),
                    self.stored_files
                        .remove(&media_full_name)
                        .unwrap_or_default(),
                ));
                self.media_file_either_owner_or_request_left
                    .remove(&media_full_name);
            }

            self.internal_send_to_controller(WebClientEvent::FileFromClient(
                TextMediaResponse::new(html_file, media_files),
                key.0,
            ));
        }
    }

    // TESTED
    fn shortcut(&self, packet: Packet) {
        match packet.routing_header.destination() {
            Some(_) => {
                self.internal_send_to_controller(WebClientEvent::Shortcut(packet));
            }
            None => {
                println!("Client {} - Packet doesn't contain a destination, it's pointless to shortcut, dropping", self.id);
            }
        }
    }

    // TESTED
    fn try_send_packet(&self, p: Packet, dest: NodeId) -> Result<Packet, Box<SendError<Packet>>> {
        let mut final_packet: Packet;
        let opt_chn: Option<&Sender<Packet>>;
        match p.pack_type {
            PacketType::FloodResponse(_) => {
                final_packet = p;
                final_packet.routing_header.increase_hop_index();
                opt_chn = self.packet_send.get(&dest);
            }

            PacketType::MsgFragment(_) | PacketType::Nack(_) | PacketType::Ack(_) => {
                (final_packet, opt_chn) = self.prepare_packet_routing(p, dest);
            }

            PacketType::FloodRequest(_) => {
                opt_chn = self.packet_send.get(&dest);
                final_packet = p;
            }
        }

        if let Some(channel) = opt_chn {
            if let Err(e) = channel.send(final_packet.clone()) {
                println!("client {} - CANNOT send packet: {e}", self.id);
                return Err(Box::new(e));
            }

            let _ = self
                .controller_send
                .send(WebClientEvent::PacketSent(final_packet.clone()));
            Ok(final_packet)
        } else {
            Err(Box::new(SendError(final_packet)))
        }
    }

    // TESTED
    fn is_correct_server_type(&self, server_id: NodeId, requested_type: &GraphNodeType) -> bool {
        self.nodes_type
            .get(&server_id)
            .is_some_and(|t| t == requested_type)
    }

    /*
       Given a packet, id and destination it searches for a path in the graph and returns
       an updated packet with the new header and an optional channel.
       if no path exist or the channel isn't open, the returned option is None
    */
    // TESTED
    fn prepare_packet_routing(
        &self,
        mut packet: Packet,
        dest: NodeId,
    ) -> (Packet, Option<&Sender<Packet>>) {
        packet.routing_header = SourceRoutingHeader::empty_route();

        let opt_header = astar(
            &self.topology_graph,
            self.id,
            |n| n == dest,
            |(_, _, weight)| *weight,
            |_| 0.,
        )
        .map(|(_, path)| wg_2024::network::SourceRoutingHeader::with_first_hop(path));

        if let Some(routing_header) = opt_header {
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

    // TESTED
    fn client_is_destination(&self, p: &Packet) -> bool {
        p.routing_header
            .destination()
            .is_some_and(|dest| dest == self.id)
    }

    // TESTED
    fn get_request_index(&self, p: &Packet) -> Option<usize> {
        self.pending_requests
            .iter()
            .position(|req| req.request_id == PacketId::from_u64(p.session_id).get_request_id())
    }

    // TESTED
    fn add_new_edge(&mut self, from: NodeId, to: NodeId, weight: f64) {
        if !self.topology_graph.contains_edge(from, to) {
            self.topology_graph.add_edge(from, to, weight);
        }
    }

    // TESTED
    fn remove_node(&mut self, node_to_remove: NodeId) {
        if self
            .nodes_type
            .get(&node_to_remove)
            .is_none_or(|t| *t == GraphNodeType::Drone)
        {
            self.topology_graph.remove_node(node_to_remove);
            self.nodes_type.remove(&node_to_remove);
            self.packets_sent_counter.remove(&node_to_remove);
        }
    }

    // TESTED
    fn update_graph_weight(&mut self, drone_id: NodeId) {
        let new_weight = self
            .packets_sent_counter
            .get(&drone_id)
            .map_or(1., |(d, n)| 1. / (1. - (*n / *d)));

        for (_, _, weight) in self
            .topology_graph
            .all_edges_mut()
            .filter(|(_, to, _)| *to == drone_id)
        {
            *weight = new_weight;
        }
    }

    // TESTED
    fn update_packet_counter_after_nack(&mut self, header: &SourceRoutingHeader, problematic_node: NodeId) {
        for id in &header.hops {
            if *id == problematic_node {
                break;
            }

            self.packets_sent_counter
                .entry(*id)
                .and_modify(|(sent, _)| *sent += 1.);
        }

        self.packets_sent_counter
            .entry(problematic_node)
            .and_modify(|(sent, lost)| {
                *sent += 1.;
                *lost += 1.;
            });

        self.update_graph_weight(problematic_node);
    }

    // TESTED
    fn update_packet_counter_after_ack(&mut self, header: &SourceRoutingHeader) {
        for drone_id in &header.hops {
            self.packets_sent_counter
                .entry(*drone_id)
                .and_modify(|(sent, _)| *sent += 1.);

            self.update_graph_weight(*drone_id);
        }
    }

    // TESTED
    fn handle_flood_response(&mut self, packet: Packet, resp: &FloodResponse) {
        let initiator: Option<&(NodeId, NodeType)> = resp.path_trace.first();

        if initiator.is_none() {
            println!(
                "client {} - Received a flood response with empty path trace, dropping",
                self.id
            );
            return;
        }

        if initiator.unwrap().0 == self.id {
            if resp.flood_id != self.sequential_flood_id {
                println!(
                    "client {} - received a flood response started by me with wrong flood_id, dropping",
                    self.id
                );
                return;
            }
            let mut prev: Option<(NodeId, NodeType)> = None;
            for (id, node_type) in &resp.path_trace {
                if let Some((from_id, from_type)) = prev {
                    if *id == self.id || from_id == self.id {
                        self.add_new_edge(from_id, *id, DEFAULT_WEIGHT);
                        self.add_new_edge(*id, from_id, DEFAULT_WEIGHT);
                    } else {
                        // this prevents A* to find path with client/server in the middle
                        if matches!(from_type, NodeType::Client)
                            | matches!(from_type, NodeType::Server)
                        {
                            self.add_new_edge(*id, from_id, DEFAULT_WEIGHT);
                        } else if matches!(node_type, NodeType::Client)
                            | matches!(node_type, NodeType::Server)
                        {
                            self.add_new_edge(from_id, *id, DEFAULT_WEIGHT);
                        } else {
                            self.add_new_edge(from_id, *id, DEFAULT_WEIGHT);
                            self.add_new_edge(*id, from_id, DEFAULT_WEIGHT);
                        }
                    }

                    self.nodes_type
                        .insert(*id, (*node_type).into());

                    // initialize the packet counter
                    for (drone_id, _) in self
                        .nodes_type
                        .iter()
                        .filter(|(_, t)| **t == GraphNodeType::Drone)
                    {
                        if !self.packets_sent_counter.contains_key(drone_id) {
                            self.packets_sent_counter.insert(*drone_id, (0., 0.));
                        }
                    }
                }
                prev = Some((*id, *node_type));
            }

            println!(
                "client: {} - graph: {:?} - nodes type: {:?}",
                self.id, self.topology_graph, self.nodes_type
            );
        } else if let Some(next_hop_drone_id) = packet.routing_header.next_hop() {
            if let Err(e) = self.try_send_packet(packet, next_hop_drone_id) {
                // I don't have the channel to forward the flood response - SHORTCUT
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
                let packet_id = PacketId::from_u64(packet.session_id);
                if req.waiting_for_ack.remove(&packet_id).is_some() {
                    if let Some(header) = self.routing_header_history.remove(&packet_id) {
                        self.update_packet_counter_after_ack(&header);
                    }
                } else {
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

    // TESTED
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

                let original_header = self
                    .routing_header_history
                    .remove(&PacketId::from_u64(packet.session_id));

                match nack.nack_type {
                    NackType::Dropped | NackType::DestinationIsDrone => {
                        let dest = req.server_id;
                        let new_packet = Packet::new_fragment(
                            SourceRoutingHeader::empty_route(),
                            packet.session_id,
                            fragment.clone(),
                        );

                        // update edges weight
                        if let NackType::Dropped = nack.nack_type {
                            if let Some(header) = original_header{
                                if let Some(source) = packet.routing_header.source() {
                                    self.update_packet_counter_after_nack(&header, source);
                                }
                            }
                        }

                        if self.try_send_packet(new_packet.clone(), dest).is_err() {
                            self.packets_to_bo_sent_again.push_back((
                                PacketId::from_u64(packet.session_id),
                                fragment.clone(),
                            ));
                            self.start_flooding();
                        }
                    }

                    NackType::ErrorInRouting(node_to_remove)
                    | NackType::UnexpectedRecipient(node_to_remove) => {
                        let dest = req.server_id;

                        // remove problematic drone and search for a new path.
                        // if found send, otherwise start a flood
                        self.remove_node(node_to_remove);

                        let p = Packet::new_fragment(
                            SourceRoutingHeader::empty_route(),
                            packet.session_id,
                            fragment.clone(),
                        );
                        if self.try_send_packet(p, dest).is_err() {
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

    // TESTED
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

    // TESTED
    fn complete_request_with_generic_response(
        &mut self,
        server_id: NodeId,
        request_type: RequestType,
        resp: &GenericResponse,
    ) {
        match resp {
            GenericResponse::Type(server_type) => {
                self.nodes_type
                    .entry(server_id)
                    .and_modify(|t| *t = (*server_type).into());

                // if I discovered all the server type
                if !self
                    .nodes_type
                    .iter()
                    .any(|(_, t)| matches!(t, GraphNodeType::Server))
                {
                    let mut list = HashMap::new();
                    for (id, t) in &self.nodes_type {
                        match t {
                            GraphNodeType::ChatServer => {
                                list.insert(*id, ServerType::ChatServer);
                            }
                            GraphNodeType::MediaServer => {
                                list.insert(*id, ServerType::MediaServer);
                            }
                            GraphNodeType::TextServer => {
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

    // TESTED
    fn complete_request_with_text_response(
        &mut self,
        server_id: NodeId,
        request_type: RequestType,
        resp: TextResponse,
    ) {
        match resp {
            TextResponse::TextList(vec) => {
                println!("sending message to scl {{{vec:?}}}");
                self.internal_send_to_controller(WebClientEvent::ListOfFiles(vec, server_id));
            }
            TextResponse::Text(file) => {
                let RequestType::Text(text_path, _) = request_type else {
                    println!("Response is not coherent with request, dropping request");
                    return;
                };

                let file_str = String::from_utf8(file.clone()).unwrap();

                let needed_media = get_media_inside_html_file(&file_str);

                if needed_media.is_empty() {
                    self.internal_send_to_controller(WebClientEvent::FileFromClient(
                        TextMediaResponse::new(
                            (get_filename_from_path(&text_path), file),
                            Vec::new(),
                        ),
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

                    // for every media:
                    // if present in cache do nothing
                    // else, if I know the owner, do nothing (the file is arriving - I have already requeted it when I discovered its owner)
                    // else, if remaining list counter is set, do nothing(it's already been asked)
                    // else, set counter and ask media lists

                    for media_path in needed_media {
                        if self.stored_files.contains_key(&media_path) {
                            continue;
                        }

                        if let std::collections::hash_map::Entry::Vacant(e) = self
                            .media_file_either_owner_or_request_left
                            .entry(media_path)
                        {
                            e.insert(Either::Right(
                                self.nodes_type
                                    .iter()
                                    .filter(|(_, t)| **t == GraphNodeType::MediaServer)
                                    .map(|(id, _)| *id)
                                    .collect(),
                            ));
                            is_required_media_list_request = true;
                        }

                        println!(
                            "client {} - {:?}",
                            self.id, self.media_file_either_owner_or_request_left
                        );
                    }

                    if is_required_media_list_request {
                        // create media list request
                        self.nodes_type
                            .iter()
                            .filter(|(_, t)| **t == GraphNodeType::MediaServer)
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

    // TESTED
    fn complete_request_with_media_response(
        &mut self,
        server_id: NodeId,
        request_type: RequestType,
        resp: MediaResponse,
    ) {
        match resp {
            MediaResponse::MediaList(file_list) => {
                // for every needed file, remove the media server from file's list of requests
                for either in &mut self.media_file_either_owner_or_request_left.values_mut() {
                    if let Either::Right(media_server_list) = either {
                        if let Some(idx) = media_server_list.iter().position(|id| *id == server_id)
                        {
                            media_server_list.remove(idx);
                            if media_server_list.is_empty() {
                                *either = Either::Left(None);
                            }
                        }
                    }
                }

                // for every file in the list *that is needed*, if owner not set, set it and create request
                // else, do nothing (the file is arriving)
                for media_path in file_list {
                    let file_is_needed = self
                        .text_media_map
                        .values()
                        .any(|v| v.contains(&media_path));

                    let owner_is_set = self
                        .media_file_either_owner_or_request_left
                        .get(&media_path)
                        .is_some_and(|either| {
                            if let Either::Left(owner) = either {
                                return owner.is_some();
                            }
                            false
                        });

                    if file_is_needed && !owner_is_set {
                        self.media_file_either_owner_or_request_left
                            .insert(media_path.clone(), Either::Left(Some(server_id)));
                        self.create_request(RequestType::Media(media_path, server_id));
                    }
                }
            }
            MediaResponse::Media(file) => {
                let RequestType::Media(media_path, _) = request_type else {
                    println!("Response is not coherent with request, dropping request");
                    return;
                };

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
                        &resp,
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

    // TESTED
    fn handle_command(&mut self, command: WebClientCommand) {
        println!("Handling command {command:?}");
        match command {
            WebClientCommand::AddSender(id, sender) => {
                self.packet_send.insert(id, sender);
                self.add_new_edge(self.id, id, DEFAULT_WEIGHT);
                self.add_new_edge(id, self.id, DEFAULT_WEIGHT);
                self.start_flooding();
            }

            WebClientCommand::RemoveSender(id) => {
                self.remove_node(id);
                self.packet_send.remove(&id);
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

    // TESTED
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

        for dest in &self.packet_send {
            let _ = self.try_send_packet(packet.clone(), *dest.0);
        }
    }

    // TESTED
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
            let packet = wg_2024::packet::Packet::new_fragment(
                SourceRoutingHeader::empty_route(),
                self.packet_id_counter.get_session_id(),
                f.clone(),
            );
            new_request
                .waiting_for_ack
                .insert(self.packet_id_counter.clone(), f.clone());

            if let Ok(packet_sent) = self.try_send_packet(packet, server_id) {
                self.routing_header_history.insert(
                    PacketId::from_u64(packet_sent.session_id),
                    packet_sent.routing_header.clone(),
                );
            } else {
                self.packets_to_bo_sent_again
                    .push_back((self.packet_id_counter.clone(), f));
                self.start_flooding();
            }
            self.packet_id_counter.increment_packet_id();
        }

        self.pending_requests.push(new_request);

        self.packet_id_counter.increment_request_id();
    }

    // TESTED
    fn create_request(&mut self, request_type: RequestType) {
        let compression: Compression; // TODO has to be chosen by scl or randomically

        match &request_type {
            RequestType::TextList(server_id) => {
                if !self.is_correct_server_type(*server_id, &GraphNodeType::TextServer) {
                    self.internal_send_to_controller(WebClientEvent::UnsupportedRequest);
                    return;
                }

                compression = Compression::None;
                let frags =
                    web_messages::RequestMessage::new_text_list_request(self.id, compression.clone())
                        .fragment()
                        .expect("Error during fragmentation. This can't happen. If it happens there is a bug somwhere");

                self.add_request(*server_id, compression, frags, request_type);
            }

            RequestType::MediaList(server_id) => {
                if !self.is_correct_server_type(*server_id, &GraphNodeType::MediaServer) {
                    self.internal_send_to_controller(WebClientEvent::UnsupportedRequest);
                    return;
                }

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
                if !self.is_correct_server_type(*server_id, &GraphNodeType::MediaServer) {
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
                if !self.is_correct_server_type(*server_id, &GraphNodeType::TextServer) {
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
