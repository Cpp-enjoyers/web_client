use common::networking::flooder::Flooder;
use common::ring_buffer::RingBuffer;
use common::slc_commands::{ServerType, TextMediaResponse, WebClientCommand, WebClientEvent};
use common::web_messages;
use common::web_messages::{
    Compression, GenericResponse, MediaResponse, RequestMessage, Response, ResponseMessage,
    Serializable, SerializableSerde, SerializationError, TextResponse,
};
use common::Client;
use compression::huffman::HuffmanCompressor;
use compression::lzw::LZWCompressor;
use compression::Compressor;
use core::time;
use crossbeam_channel::{select_biased, Receiver, SendError, Sender};
use flooding::RING_BUFF_SZ;
use itertools::Either;
use log::{error, info};
use petgraph::algo::astar;
use petgraph::prelude::DiGraphMap;
use std::collections::{HashMap, VecDeque};
use std::thread::sleep;
use std::vec;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{
    FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType,
    FRAGMENT_DSIZE,
};

use crate::utils::{get_filename_from_path, get_media_inside_html_file, PacketId, RequestId};

mod flooding;

#[cfg(test)]
mod test;
#[cfg(test)]
mod utils_for_test;

// Default value put inside an edge in the topology graph
const DEFAULT_WEIGHT: f64 = 0.5;

// represents a node's type
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

// trait that represents the capability of fragmenting an object. (i.e. to split it in WG Fragments)
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

// represents all the request types that the web client can accept

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestType {
    TextList(NodeId),
    MediaList(NodeId),
    ServersType,
    Text(String, NodeId),
    Media(String, NodeId),
}

/*
    Represent a request created by the client.
    It keeps track of the packets that have been sent and are waiting for tha ACK
    and also of the packets that compose the response from the server
*/
#[derive(Debug, Clone, PartialEq)]
struct WebBrowserRequest {
    // Requets's ID
    request_id: RequestId,
    // Destination's ID
    server_id: NodeId,
    // stores the outgoing fragments that are still waiting for ACK
    waiting_for_ack: HashMap<PacketId, Fragment>,
    // stores the incoming fragments that compose the response of the query
    incoming_messages: Vec<Fragment>,
    // compression type of the response
    compression: Compression,
    // request's type
    request_type: RequestType,
    // set to true when all of the response's fragments have been received
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
/**
    Struct that represent the web client

    When the scl asks for a text file, the client downloads it from the server, performs a parsing
    to understand if media files are linked inside it and, if necessary, retrieves them from the correct server.
    If some media are unavailable, the text file and the available media are sent anyways.

    The topology is stored inside a graph structure. The client tries to estimate the PDR of every drone
    in order to choose the best path for each message

    The client can ask for compresed or uncompressed data. Together with the web servers, it implements 2 different compression algorithms:
    - LZW
    - Huffman
*/
#[derive(Debug)]
pub struct WebBrowser {
    // web client's ID in the network
    id: NodeId,
    // prefix for logs with node ID
    log_prefix: String,
    // channel to scl
    controller_send: Sender<WebClientEvent>,
    // channel from scl
    controller_recv: Receiver<WebClientCommand>,
    // channel to myself
    packet_recv: Receiver<Packet>,
    // channles to my direct neighbors
    packet_send: HashMap<NodeId, Sender<Packet>>,
    // stores the floodIDs of received floods initiated by a certain drone
    flood_history: HashMap<NodeId, RingBuffer<u64>>,
    // id used to generate my floods
    sequential_flood_id: u64,
    // array of requests that the client is waiting to complete
    pending_requests: Vec<WebBrowserRequest>,
    // incremental id that embeds both the requestID and the packetID. This is used to easily handle the WG's sessionID
    packet_id_counter: PacketId,
    // Directed graph that represents the topology. To avoid client/servers being in the middle of a path, this stores only edges outgoing from clients/servers
    topology_graph: DiGraphMap<NodeId, f64>,
    // keeps track of each node's type
    nodes_type: HashMap<NodeId, GraphNodeType>,
    // stores the outgoing packets for which I couldn't find a path or I received a NACK back. They need to be sent again
    packets_to_bo_sent_again: VecDeque<(PacketId, Fragment)>,
    // links a text filename and the nodeId that provided it to the media filenames that it requires
    text_media_map: HashMap<(NodeId, String), Vec<String>>,
    // stores the text/media file that are needed by an open request
    stored_files: HashMap<String, Vec<u8>>,
    // stores either the media's owner or the list of media servers that still need to respond to the file list request
    media_file_either_owner_or_request_left: HashMap<String, Either<Option<NodeId>, Vec<NodeId>>>,
    // keeps track the number of packets (sent, lost) through every drone. Used to calculate edge weight in the graph
    packets_sent_counter: HashMap<NodeId, (f64, f64)>,
    // keeps track of the path of each packet until ack or nack are received, in order to correctly handle packets_sent_counter
    routing_header_history: HashMap<PacketId, SourceRoutingHeader>,
}

impl Client<WebClientCommand, WebClientEvent> for WebBrowser {
    /**
        Web client's constructor
        - id: Client's ID in the network
        - `controller_send`: Channel towards the scl
        - `controller_recv`: Channel from the scl
        - `packet_recv`: Channel to myself, used to listen for packets
        - `packet_send`: Hashmap that links each neighbor's ID woth its channel
    */
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
            log_prefix: format!("Web Client[{id}]"),
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

    /**
        Core function that "turns on" the client.

        This puts the client in an infinite loop where it reads messages from both scl and neighbors.
        at each iteration it also looks for a pending request that is ready to be completed and it tries
        to send again a packet that has not been acknowledged.
    */
    fn run(&mut self) {
        info!(target: &self.log_prefix, "Web client is running");
        sleep(time::Duration::from_millis(100));
        self.start_flooding();

        loop {
            // look for a complete request and, if found, finalize it
            self.try_complete_request();

            // try sending again a packet that received a NACK
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
    // takes a client event as a parameter and sends it to the scl
    fn internal_send_to_controller(&self, msg: &WebClientEvent) {
        if let Err(e) = self.controller_send.send(msg.clone()) {
            error!(target: &self.log_prefix, "internal_send_to_controller: Cannot send message to scl: {e:?}");
        } else {
            info!(target: &self.log_prefix, "internal_send_to_controller: Message sent to scl: {:?}", msg);
        }
    }

    // if the first packet in the queue can be sent again it sends it, otherwise it pushes it at the end of the queue
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
                        // store the header in order to correctly update the packet_sent_counter
                        self.routing_header_history.insert(
                            PacketId::from_u64(packet_sent.session_id),
                            packet_sent.routing_header.clone(),
                        );

                        info!(target: &self.log_prefix, "internal_send_to_controller: A message that received a NACK has been sent again: {:?}", packet_sent);
                    }

                    Err(e) => {
                        self.packets_to_bo_sent_again
                            .push_back((PacketId::from_u64(e.0.session_id), frag));

                        info!(target: &self.log_prefix, "internal_send_to_controller: A message thta received a NACK couldn't be sent again: {:?}", e.into_inner());
                    }
                }
            } else {
                error!(target: &self.log_prefix, "internal_send_to_controller: Found a message to be sent again without the corresponding pending request. BUG");
            }
        }
    }

    // search for a request that is ready to be completed and finilises it.
    // it also searches for a text file that has received all the media files
    // and sends the result to scl
    fn try_complete_request(&mut self) {
        if let Some(i) = self
            .pending_requests
            .iter()
            .position(|req| req.response_is_complete && req.waiting_for_ack.is_empty())
        {
            info!(target: &self.log_prefix, "try_complete_request: Found a request ready to be completed");
            let req = self.pending_requests.remove(i);
            self.complete_request(req);
        }

        // search for a text file whose needed media are either unavailable or already arrived to the client
        if let Some(key) = self
            .text_media_map
            .iter()
            .find(|(_, list)| {
                for f in *list {
                    // owner can still be discovered if media_file_either_owner_or_request_left contains a non-empty list of media servers
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

                    let owner_is_available = self
                        .media_file_either_owner_or_request_left
                        .get(f)
                        .is_some_and(|either| {
                            if let itertools::Either::Left(owner) = either {
                                return owner.is_some();
                            }
                            false
                        });

                    if !media_is_already_stored
                        && (owner_can_still_be_discovered || owner_is_available)
                    {
                        return false;
                    }
                }
                true
            })
            .map(|(key, _)| key.clone())
        {
            info!(target: &self.log_prefix, "try_complete_request: Found a text file with all its media");
            self.send_text_and_media_back(&key);
        }
    }

    // given a key for text_media_map, it retrieves the text file and the related media and sends everything to scl
    fn send_text_and_media_back(&mut self, key: &(NodeId, String)) {
        if let Some(media_list) = self.text_media_map.remove(key) {
            let html_file = (
                get_filename_from_path(&key.1),
                self.stored_files.remove(&key.1).unwrap_or_default(),
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

            if html_file.1.is_empty() {
                error!(target: &self.log_prefix, "send_text_and_media_back: The text file {} was not correctly stored, the entire request has to be dropped", key.1);
                return;
            }

            info!(target: &self.log_prefix, "send_text_and_media_back: Sending to scl the text file {:?} and all its needed media {:?}", html_file, media_files);

            self.internal_send_to_controller(&WebClientEvent::FileFromClient(
                TextMediaResponse::new(html_file, media_files),
                key.0,
            ));
        }
    }

    // the given packet is shortcutted to the scl
    fn shortcut(&self, packet: Packet) {
        match packet.routing_header.destination() {
            Some(_) => {
                self.internal_send_to_controller(&WebClientEvent::Shortcut(packet));
            }
            None => {
                error!(target: &self.log_prefix, "shortcut: Packet doesn't contain a destination, it's pointless to shortcut, dropping");
            }
        }
    }

    // it tries to send the packet p to the node whose ID is dest
    // an error is returned if the channel for the first hop is not available or a path does not exist
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
                error!(target: &self.log_prefix, "try_send_packet: Error while sending to {dest} the packet: {e:?}");
                return Err(Box::new(e));
            }

            self.internal_send_to_controller(&WebClientEvent::PacketSent(final_packet.clone()));

            info!(target: &self.log_prefix, "try_send_packet: Packet correctly sent the packet with ID: {} to {dest}", final_packet.session_id);

            Ok(final_packet)
        } else {
            error!(target: &self.log_prefix, "try_send_packet: Channel not found towards node: {dest} the packet: {final_packet:?}");

            Err(Box::new(SendError(final_packet)))
        }
    }

    // checks that node_id's type is equal to requested_type
    fn is_correct_server_type(&self, node_id: NodeId, requested_type: &GraphNodeType) -> bool {
        self.nodes_type
            .get(&node_id)
            .is_some_and(|t| t == requested_type)
    }

    /*
       Given a packet, id and destination it searches for a path in the graph and returns
       an updated packet with the new header and an optional channel.
       if no path exist or the channel isn't open, the returned option is None
    */
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
                    info!(target: &self.log_prefix, "prepare_packet_routing: found a path towards node: {dest:?}");

                    return (packet, Some(channel));
                }
            }
        }
        error!(target: &self.log_prefix, "prepare_packet_routing: did not found a path towards node: {dest:?} to send packet: {packet:?}");

        (packet, None)
    }

    // checks that this web client is the destination of packet p
    fn client_is_destination(&self, p: &Packet) -> bool {
        p.routing_header
            .destination()
            .is_some_and(|dest| dest == self.id)
    }

    // given a packet p, it retrieves the position of the corresponding request inside the pending_request array
    fn get_request_index(&self, p: &Packet) -> Option<usize> {
        self.pending_requests
            .iter()
            .position(|req| req.request_id == PacketId::from_u64(p.session_id).get_request_id())
    }

    // adds the edge directed (from, to, weight) to the graph only if it doesn't already exist
    fn add_new_edge(&mut self, from: NodeId, to: NodeId, weight: f64) {
        if !self.topology_graph.contains_edge(from, to) {
            self.topology_graph.add_edge(from, to, weight);
            info!(target: &self.log_prefix, "add_new_edge: Added the edge from {from} to {to}");
        }
    }

    // removes the node "node_to_remove" from graph and also from nodes_type and packet_sent_counter to keep consistency between the data structures
    fn remove_node(&mut self, node_to_remove: NodeId) {
        if self
            .nodes_type
            .get(&node_to_remove)
            .is_none_or(|t| *t == GraphNodeType::Drone)
        {
            self.topology_graph.remove_node(node_to_remove);
            self.nodes_type.remove(&node_to_remove);
            self.packets_sent_counter.remove(&node_to_remove);
            info!(target: &self.log_prefix, "remove_node: Removed the node {node_to_remove} from graph");
        }
    }

    // updates the weight of every incoming edge from the node "drone_id"
    // the used metric is 1 / (1 - (lost / sent))
    fn update_graph_weight(&mut self, drone_id: NodeId) {
        let new_weight = self
            .packets_sent_counter
            .get(&drone_id)
            .map_or(1., |(d, n)| 1. / (1. - (*n / *d)));

        for (from, to, weight) in self
            .topology_graph
            .all_edges_mut()
            .filter(|(_, to, _)| *to == drone_id)
        {
            *weight = new_weight;
            info!(target: &self.log_prefix, "update_graph_weight: Edge from {from} to {to} has now weight of {new_weight}");
        }
    }

    // when a nack::dropped is received, the packet counter and graph are updated to take into account this error:
    // every node from myself to the problematic_node that dropped the message gets +1 to the counter of sent message
    // the drone that has dropped it gets +1 to both sent and lost messages.
    // finally, the graph is updated with the new weights
    // header contains the path that the packet travelled from me to problematic_node
    fn update_packet_counter_nack_dropped(
        &mut self,
        header: &SourceRoutingHeader,
        problematic_node: NodeId,
    ) {
        for id in &header.hops {
            if *id == problematic_node {
                break;
            }

            self.packets_sent_counter
                .entry(*id)
                .and_modify(|(sent, _)| *sent += 1.);

            self.update_graph_weight(*id);

            info!(target: &self.log_prefix, "update_packet_counter_after_nack: Updating sent counter of {id}");
        }

        self.packets_sent_counter
            .entry(problematic_node)
            .and_modify(|(sent, lost)| {
                *sent += 1.;
                *lost += 1.;
            });
        info!(target: &self.log_prefix, "update_packet_counter_after_nack: Updating sent and lost counter of {problematic_node}");

        self.update_graph_weight(problematic_node);
    }

    // when an ack, the packet counter and graph are updated to take into account this event:
    // every node inside the path gets +1 to the counter of sent message
    // finally, the graph is updated with the new weights
    // header contains the path that the packet travelled from me to destination
    fn update_packet_counter_ack(&mut self, header: &SourceRoutingHeader) {
        for drone_id in &header.hops {
            self.packets_sent_counter
                .entry(*drone_id)
                .and_modify(|(sent, _)| *sent += 1.);

            info!(target: &self.log_prefix, "update_packet_counter_after_ack: Updating sent counter of {drone_id}");

            self.update_graph_weight(*drone_id);
        }
    }

    // if the flood response is mine and is not old, the graph can be updated
    // to make simpler the path finding procedure, the graph is directed and each client/server
    // have only outgoing edges (they are sink of the graph), in this way the path cannot go trough clients/servers
    fn handle_flood_response(&mut self, packet: Packet, resp: &FloodResponse) {
        let initiator: Option<&(NodeId, NodeType)> = resp.path_trace.first();

        if initiator.is_none() {
            error!(target: &self.log_prefix, "handle_flood_response: Received a flood response with empty path trace, dropping");
            return;
        }

        if initiator.unwrap().0 == self.id {
            if resp.flood_id != self.sequential_flood_id {
                info!(target: &self.log_prefix, "handle_flood_response: Received an old flood response, ignoring");
                return;
            }
            info!(target: &self.log_prefix, "handle_flood_response: Received a flood response, updating graph...");

            let mut prev: Option<(NodeId, NodeType)> = None;
            for (to_id, to_type) in &resp.path_trace {
                if let Some((from_id, from_type)) = prev {
                    if *to_id == self.id || from_id == self.id {
                        self.add_new_edge(from_id, *to_id, DEFAULT_WEIGHT);
                        self.add_new_edge(*to_id, from_id, DEFAULT_WEIGHT);
                    } else {
                        // this prevents A* to find path with client/server in the middle
                        if matches!(from_type, NodeType::Client)
                            | matches!(from_type, NodeType::Server)
                        {
                            self.add_new_edge(*to_id, from_id, DEFAULT_WEIGHT);
                        } else if matches!(to_type, NodeType::Client)
                            | matches!(to_type, NodeType::Server)
                        {
                            self.add_new_edge(from_id, *to_id, DEFAULT_WEIGHT);
                        } else {
                            self.add_new_edge(from_id, *to_id, DEFAULT_WEIGHT);
                            self.add_new_edge(*to_id, from_id, DEFAULT_WEIGHT);
                        }
                    }

                    self.nodes_type.insert(*to_id, (*to_type).into());

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
                prev = Some((*to_id, *to_type));
            }
        } else if let Some(next_hop_drone_id) = packet.routing_header.next_hop() {
            if let Err(e) = self.try_send_packet(packet, next_hop_drone_id) {
                info!(target: &self.log_prefix, "handle_flood_response: I coudln't find a path to forward the flood response - SHORTCUT");

                // I don't have the channel to forward the flood response - SHORTCUT
                self.shortcut(e.0);
            }
        } else {
            error!(target: &self.log_prefix, "handle_flood_response: Found a flood response with a corrupted routing header which it's pointless to shortcut, dropping");
        }
    }

    // if the ack is for me, search for the corresponding request and remove the packet from the waiting_for_ack list
    // and update packet_set_counter
    fn handle_ack(&mut self, packet: Packet) {
        if !self.client_is_destination(&packet) {
            error!(target: &self.log_prefix, "handle_ack: Received an ack that is not for me, shortcut");
            self.shortcut(packet);
            return;
        }

        match self.get_request_index(&packet) {
            Some(id) => {
                let req = self.pending_requests.get_mut(id).unwrap();
                let packet_id = PacketId::from_u64(packet.session_id);
                if req.waiting_for_ack.remove(&packet_id).is_some() {
                    if let Some(header) = self.routing_header_history.remove(&packet_id) {
                        self.update_packet_counter_ack(&header);
                        info!(target: &self.log_prefix, "handle_ack: ack correctly elaborated {packet:?}");
                    }
                } else {
                    info!(target: &self.log_prefix, "handle_ack: I received an ack for a packet that has already been acknowledged: {packet:?}");
                }
            }
            None => {
                info!(target: &self.log_prefix, "handle_ack: I received an ack for an unknown req_id, dropping: {packet:?}");
            }
        }
    }

    // if the fragment is for me, find the corresponding request and store packet fragmetn inside the response array
    // and send the ack back to the server
    fn handle_fragment(&mut self, packet: Packet, fragment: &Fragment) {
        if !self.client_is_destination(&packet) {
            error!(target: &self.log_prefix, "handle_fragment: Received a fragment that is not for me, shortcut");
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
                    error!(target: &self.log_prefix, "handle_fragment: I received the same fragment multiple times, bug, ignoring the message");
                    return;
                }

                req.incoming_messages.push(fragment.clone());

                if req.incoming_messages.len() == n_frags {
                    // I have all the fragments
                    req.response_is_complete = true;
                    info!(target: &self.log_prefix, "handle_fragment: I received all the fragment for the request {}", req.request_id);
                }

                let ack_destination = req.server_id;

                // send ACK to acknowledge the packet
                let ack = Packet::new_ack(
                    SourceRoutingHeader::empty_route(),
                    packet.session_id,
                    fragment.fragment_index,
                );

                if let Err(mut e) = self.try_send_packet(ack, ack_destination) {
                    error!(target: &self.log_prefix, "handle_fragment: Can't find a path to the node, I need to shortcut ACK");
                    e.0.routing_header =
                        SourceRoutingHeader::initialize(vec![self.id, ack_destination]);
                    self.shortcut(e.0);
                }
            }
            None => {
                info!(target: &self.log_prefix, "handle_fragment: I received a fragment for req_id \"{}\" that it's unknown to me, dropping",PacketId::from_u64(packet.session_id).get_request_id());
            }
        }
    }

    // if the nack is for me, I need to resend a packet. If the nack is dropped or destinationIsDrone, I can just recalculate the path and, if found, send it again.
    // otherwise I remove the node that is creating problems and then I search a path and send the packet.
    // In both cases, if the path is not found, I push the packet inside the queue of packets to be sent again and I start a new flood request.
    fn handle_nack(&mut self, packet: Packet, nack: &Nack) {
        if !self.client_is_destination(&packet) {
            error!(target: &self.log_prefix, "handle_nack: Received a nack that is not for me, shortcut");
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
                    error!(target: &self.log_prefix, "handle_nack: I received a NACK for packet_id \"{}\" that it's unknown to me, dropping", PacketId::from_u64(packet.session_id).get_packet_id());
                    return;
                };

                let original_header = self
                    .routing_header_history
                    .remove(&PacketId::from_u64(packet.session_id));

                let dest = req.server_id;
                match nack.nack_type {
                    NackType::Dropped | NackType::DestinationIsDrone => {
                        // update edges weight
                        if let NackType::Dropped = nack.nack_type {
                            if let Some(header) = original_header {
                                if let Some(source) = packet.routing_header.source() {
                                    self.update_packet_counter_nack_dropped(&header, source);
                                }
                            }
                        }
                    }

                    NackType::ErrorInRouting(node_to_remove)
                    | NackType::UnexpectedRecipient(node_to_remove) => {
                        // remove problematic drone and search for a new path.
                        // if found send, otherwise start a flood
                        self.remove_node(node_to_remove);
                    }
                }

                let new_packet = Packet::new_fragment(
                    SourceRoutingHeader::empty_route(),
                    packet.session_id,
                    fragment.clone(),
                );
                if self.try_send_packet(new_packet, dest).is_err() {
                    info!(target: &self.log_prefix, "handle_nack: I couldn't find a new path for the packet, starting a flood");
                    self.packets_to_bo_sent_again
                        .push_back((PacketId::from_u64(packet.session_id), fragment.clone()));
                    self.start_flooding();
                }
            }
            None => {
                info!(target: &self.log_prefix, "handle_nack: I received a NACK for req_id \"{}\" that it's unknown to me, dropping", PacketId::from_u64(packet.session_id).get_request_id());
            }
        }
    }

    // given a packet, it calls the correct function based on packet's type
    fn handle_packet(&mut self, packet: Packet) {
        info!(target: &self.log_prefix, "handle_packet: handling packet with ID: {}", packet.session_id);

        match packet.pack_type.clone() {
            PacketType::FloodRequest(mut req) => {
                let _ =
                    self.handle_flood_request(&packet.routing_header, packet.session_id, &mut req);
            }

            PacketType::FloodResponse(ref resp) => {
                self.handle_flood_response(packet, resp);
            }

            PacketType::Ack(_) => {
                self.handle_ack(packet);
            }

            PacketType::Nack(nack) => {
                self.handle_nack(packet, &nack);
            }

            PacketType::MsgFragment(fragment) => {
                self.handle_fragment(packet, &fragment);
            }
        }
    }

    // handles a generic response: if it's a server type response I store the information inside nodes_type
    // and, if I discovered all of the servers, it sends to scl the servers list
    // if it's not a server type response I send to scl an error event
    fn complete_request_with_generic_response(
        &mut self,
        server_id: NodeId,
        resp: &GenericResponse,
    ) {
        info!(target: &self.log_prefix, "complete_request_with_generic_response: I received a response of type: {:?}", resp);
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
                    info!(target: &self.log_prefix, "complete_request_with_generic_response: I discovered all the server type, preparing the response for scl");
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
                    info!(target: &self.log_prefix, "complete_request_with_generic_response: Sending the server type's list to scl");
                    self.internal_send_to_controller(&WebClientEvent::ServersTypes(list));
                }
            }
            GenericResponse::InvalidRequest | GenericResponse::NotFound => {
                self.internal_send_to_controller(&WebClientEvent::UnsupportedRequest);
            }
        }
    }

    // handles a text response: if it's a text list it sends its content to scl
    // If it's a text file response it parses it to find media file linked inside it
    // if none is found, the file is directly sent to scl, otherwise it is stored
    // and the client asks for the needed file to the media servers
    fn complete_request_with_text_response(
        &mut self,
        server_id: NodeId,
        request_type: RequestType,
        resp: TextResponse,
    ) {
        info!(target: &self.log_prefix, "complete_request_with_text_response: I received a response of type: {:?}", resp);
        match resp {
            TextResponse::TextList(vec) => {
                info!(target: &self.log_prefix, "complete_request_with_text_response: Sending text file list to scl: {{{vec:?}}}");
                self.internal_send_to_controller(&WebClientEvent::ListOfFiles(vec, server_id));
            }
            TextResponse::Text(file) => {
                let RequestType::Text(text_path, _) = request_type else {
                    error!(target: &self.log_prefix, "complete_request_with_text_response: Response is not coherent with the request, dropping");
                    return;
                };

                let file_str = String::from_utf8(file.clone()).unwrap();

                let needed_media = get_media_inside_html_file(&file_str);

                if needed_media.is_empty() {
                    info!(target: &self.log_prefix, "complete_request_with_text_response: The text file \"{text_path}\" doesn't need any media file, sending it to scl");
                    self.internal_send_to_controller(&WebClientEvent::FileFromClient(
                        TextMediaResponse::new(
                            (get_filename_from_path(&text_path), file),
                            Vec::new(),
                        ),
                        server_id,
                    ));
                } else {
                    info!(target: &self.log_prefix, "complete_request_with_text_response: The text file \"{text_path}\" needs these media files: {{{needed_media:?}}}");

                    // store the file while waiting for media
                    self.stored_files.insert(text_path.clone(), file);

                    // store media and text files links
                    self.text_media_map
                        .insert((server_id, text_path.clone()), needed_media.clone());

                    let mut is_required_media_list_request = false;

                    // for every media:
                    // if present in cache do nothing
                    // else, if I know the owner, do nothing (the file is arriving - I have already requested it when I discovered its owner)
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
                    }

                    if is_required_media_list_request {
                        // create media list request
                        info!(target: &self.log_prefix, "complete_request_with_text_response: Creating media file requests");

                        self.nodes_type
                            .iter()
                            .filter(|(_, t)| **t == GraphNodeType::MediaServer)
                            .map(|(id, _)| *id)
                            .collect::<Vec<NodeId>>()
                            .iter()
                            .for_each(|id| {
                                self.create_request(RequestType::MediaList(*id));
                            });
                    }
                }
            }
        }
    }

    // handles a media response: if it's a media list it updates the informations about
    // the media that are currently needed for some text file. If it's a media file response
    // it stores the file for later usage
    fn complete_request_with_media_response(
        &mut self,
        server_id: NodeId,
        request_type: RequestType,
        resp: MediaResponse,
    ) {
        match resp {
            MediaResponse::MediaList(file_list) => {
                // for every needed media file inside media_file_either_owner_or_request_left,
                // remove the media server from file's list of requests
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
                info!(target: &self.log_prefix, "complete_request_with_media: Updated media_file_either_owner_or_request_left is {:?}", self.media_file_either_owner_or_request_left);

                // for every file in the list *that is needed*, if owner not set then set it and create request,
                // else do nothing (the file is already arriving)
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

                        info!(target: &self.log_prefix, "complete_request_with_media: creating request to node: {server_id} for media file {media_path}");
                        self.create_request(RequestType::Media(media_path, server_id));
                    }
                }
            }
            MediaResponse::Media(file) => {
                let RequestType::Media(media_path, _) = request_type else {
                    info!(target: &self.log_prefix, "complete_request_with_media: Response is not coherent with request, dropping request");
                    return;
                };

                // store the file
                self.stored_files.insert(media_path.clone(), file);
                info!(target: &self.log_prefix, "complete_request_with_media: Received the file {media_path} from {server_id}, storing the file");
            }
        }
    }

    // completes the request passed as parameter by calling the correct function based on the response's type
    fn complete_request(&mut self, mut req: WebBrowserRequest) {
        info!(target: &self.log_prefix, "complete_request: completing req (id: {:?}, type: {:?}, to: {:?})", req.request_id, req.request_type, req.server_id);

        req.incoming_messages
            .sort_by(|f1, f2| f1.fragment_index.cmp(&f2.fragment_index));

        if let Ok(response_msg) =
            web_messages::ResponseMessage::defragment(&req.incoming_messages, req.compression)
        {
            match response_msg.content {
                Response::Generic(resp) => {
                    self.complete_request_with_generic_response(req.server_id, &resp);
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
            error!(target: &self.log_prefix, "complete_request: Cannot deserialize response, dropping");
        }
    }

    // handles a command from the scl
    fn handle_command(&mut self, command: WebClientCommand) {
        info!(target: &self.log_prefix, "handle_command: Handling command {command:?}");

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

    // starts a new flood request with a new flood_id, and it sends it to each of the direct neighbor
    fn start_flooding(&mut self) {
        self.sequential_flood_id += 1;

        info!(target: &self.log_prefix, "start_flooding: starting flood with id {}", self.sequential_flood_id);

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

    // creates the request from the given parameters and tries to send all the fragments to the destination
    // if an error occurs during the sending, the fragmetn is stored in queue of packets to be sent again
    fn add_request(
        &mut self,
        server_id: NodeId,
        compression: Compression,
        frags: Vec<Fragment>,
        request_type: RequestType,
    ) {
        let new_req_id = self.packet_id_counter.get_request_id();

        info!(target: &self.log_prefix, "add_request: adding new request to pending_request: (request_id: {new_req_id}, type: {request_type:?}, to: {server_id})");

        let mut new_request = WebBrowserRequest::new(
            new_req_id,
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
                info!(target: &self.log_prefix, "add_request: I couldn't send a packet, adding it to packets_to_bo_sent_again");

                self.packets_to_bo_sent_again
                    .push_back((self.packet_id_counter.clone(), f));
                self.start_flooding();
            }
            self.packet_id_counter.increment_packet_id();
        }

        self.pending_requests.push(new_request);

        self.packet_id_counter.increment_request_id();
    }

    // given the parameter request_type, it creates the vector of fragments that compose the request and then if calls
    // add_request() to sent the fragments and insert the new request inside the pending_request buffer
    fn create_request(&mut self, request_type: RequestType) {
        let compression: Compression;
        let frags: Vec<Fragment>;
        let dest: NodeId;

        match &request_type {
            RequestType::TextList(server_id) => {
                if !self.is_correct_server_type(*server_id, &GraphNodeType::TextServer) {
                    info!(target: &self.log_prefix, "create_request: The request type \"{request_type:?}\"is incompatible with the destination's type");
                    self.internal_send_to_controller(&WebClientEvent::UnsupportedRequest);
                    return;
                }
                dest = *server_id;
                compression = Compression::None;
                frags = web_messages::RequestMessage::new_text_list_request(
                    self.id,
                    compression.clone(),
                )
                .fragment()
                .expect(
                    "Error during fragmentation. This can't happen. If it happens there is a bug",
                );
            }

            RequestType::MediaList(server_id) => {
                if !self.is_correct_server_type(*server_id, &GraphNodeType::MediaServer) {
                    info!(target: &self.log_prefix, "create_request: The request type \"{request_type:?}\"is incompatible with the destination's type");
                    self.internal_send_to_controller(&WebClientEvent::UnsupportedRequest);
                    return;
                }
                dest = *server_id;
                compression = Compression::None;
                frags = web_messages::RequestMessage::new_media_list_request(
                    self.id,
                    compression.clone(),
                )
                .fragment()
                .expect(
                    "Error during fragmentation. This can't happen. If it happens there is a bug",
                );
            }

            RequestType::ServersType => {
                compression = Compression::None;
                let frags = RequestMessage::new_type_request(self.id, compression.clone())
                .fragment()
                .expect("Error during fragmentation. This can't happen. If it happens there is a bug");

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

                return;
            }

            RequestType::Media(file_path, server_id) => {
                compression = Compression::None;
                if !self.is_correct_server_type(*server_id, &GraphNodeType::MediaServer) {
                    info!(target: &self.log_prefix, "create_request: The request type \"{request_type:?}\"is incompatible with the destination's type");
                    self.internal_send_to_controller(&WebClientEvent::UnsupportedRequest);
                    return;
                }
                dest = *server_id;
                frags = web_messages::RequestMessage::new_media_request(
                    self.id,
                    compression.clone(),
                    file_path.clone(),
                )
                .fragment()
                .expect(
                    "Error during fragmentation. This can't happen. If it happens there is a bug",
                );
            }

            RequestType::Text(file_path, server_id) => {
                compression = Compression::LZW;
                if !self.is_correct_server_type(*server_id, &GraphNodeType::TextServer) {
                    info!(target: &self.log_prefix, "create_request: The request type \"{request_type:?}\"is incompatible with the destination's type");
                    self.internal_send_to_controller(&WebClientEvent::UnsupportedRequest);
                    return;
                }
                dest = *server_id;
                frags = web_messages::RequestMessage::new_text_request(
                    self.id,
                    compression.clone(),
                    file_path.clone(),
                )
                .fragment()
                .expect(
                    "Error during fragmentation. This can't happen. If it happens there is a bug",
                );
            }
        }
        info!(target: &self.log_prefix, "create_request: Creating a new request (to: {dest}, type: {request_type:?}");

        self.add_request(dest, compression, frags, request_type);
    }
}
