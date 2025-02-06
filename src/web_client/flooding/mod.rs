use common::{networking::flooder::Flooder, ring_buffer::RingBuffer, slc_commands::WebClientEvent};
use crossbeam_channel::Sender;
use wg_2024::{
    network::NodeId,
    packet::{NodeType, Packet},
};

use super::WebBrowser;

mod test;

pub(crate) const RING_BUFF_SZ: usize = 64;

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
        self.internal_send_to_controller(WebClientEvent::PacketSent(p));
    }
}
