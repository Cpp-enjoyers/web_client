use common::{networking::flooder::Flooder, ring_buffer::RingBuffer, slc_commands::WebClientEvent};
use crossbeam_channel::Sender;
use wg_2024::{
    network::NodeId,
    packet::{NodeType, Packet},
};

use super::WebBrowser;

#[cfg(test)]
mod test;

pub(crate) const RING_BUFF_SZ: usize = 64;

impl Flooder for WebBrowser {
    const NODE_TYPE: NodeType = NodeType::Client;

    /// retrieves the node's ID
    fn get_id(&self) -> NodeId {
        self.id
    }

    /// retrieves the node's direct neighbors list
    fn get_neighbours(&self) -> impl ExactSizeIterator<Item = (&NodeId, &Sender<Packet>)> {
        self.packet_send.iter()
    }

    /// checks if the flood request identified by `flood_id` has already been received
    fn has_seen_flood(&self, flood_id: (NodeId, u64)) -> bool {
        match self.flood_history.get(&flood_id.0) {
            Some(set) => set.contains(&flood_id.1),
            None => false,
        }
    }

    /// insert `flood_id` inside the buffer of flood requests that have passed through this node
    fn insert_flood(&mut self, flood_id: (NodeId, u64)) {
        if let Some(set) = self.flood_history.get_mut(&flood_id.0) {
            set.insert(flood_id.1);
        } else {
            let mut rb = RingBuffer::with_capacity(RING_BUFF_SZ);
            rb.insert(flood_id.1);
            self.flood_history.insert(flood_id.0, rb);
        }
    }

    /// sends packet p to scl
    fn send_to_controller(&self, p: Packet) {
        self.internal_send_to_controller(&WebClientEvent::PacketSent(p));
    }
}
