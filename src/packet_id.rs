const REQ_ID_MASK: u64 = 0xFFFF;
const PACKET_ID_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

const _PACKET_ID_LEN: u8 = 48;
const REQ_ID_LEN: u8 = 16;

pub type SessionId = u64;
pub type RequestId = u16;

#[derive(Debug, Hash, Clone, PartialEq, Eq)]

/*
    Since NACK doesn't contain the fragment_index I need to store it somewhere
    this wrapper for the session_id uses the 16 LSB for the id of the request
    and the remaining 48 for the packet_id (the actual session_id)
*/

pub struct PacketId {
    packet_id: SessionId, // only 48 LSB used
    req_id: RequestId,
}
impl PacketId {
    pub fn new() -> Self {
        Self {
            packet_id: 0,
            req_id: 0,
        }
    }

    pub fn from_u64(n: u64) -> Self {
        Self {
            packet_id: n >> REQ_ID_LEN,
            req_id: (n & REQ_ID_MASK) as u16,
        }
    }

    pub fn get_packet_id(&self) -> u64 {
        self.packet_id
    }

    pub fn get_request_id(&self) -> u16 {
        self.req_id
    }

    pub fn get_session_id(&self) -> u64 {
        (self.packet_id << REQ_ID_LEN) | u64::from(self.req_id)
    }

    pub fn increment_packet_id(&mut self) {
        self.packet_id += 1;
        self.packet_id &= PACKET_ID_MASK;
    }

    pub fn increment_request_id(&mut self) {
        self.packet_id = 0; // new request has to reset the session id
        self.req_id = self.req_id.wrapping_add(1);
    }
}

#[cfg(test)]
mod packet_id_test {
    use super::PacketId;

    #[test]
    fn test_from_64() {
        let v = PacketId::from_u64(0x1234_5678_90AB_CDEF);
        assert_eq!(v.req_id, 0xCDEF);
        assert_eq!(v.packet_id, 0x0000_1234_5678_90AB);
    }

    #[test]
    fn test_getters() {
        let v = PacketId::from_u64(0x1234_5678_90AB_CDEF);
        assert_eq!(v.get_request_id(), 0xCDEF);
        assert_eq!(v.get_packet_id(), 0x0000_1234_5678_90AB);
        assert_eq!(v.get_session_id(), 0x1234_5678_90AB_CDEF);
    }

    #[test]
    fn test_increment_packet() {
        let mut v = PacketId::from_u64(0x1234_5678_90AB_CDEF);
        v.increment_packet_id();
        assert_eq!(v.get_packet_id(), 0x0000_1234_5678_90AC);
        assert_eq!(v.get_session_id(), 0x1234_5678_90AC_CDEF);
    }

    #[test]
    fn test_increment_req() {
        let mut v = PacketId::from_u64(0x1234_5678_90AB_CDEF);
        v.increment_request_id();
        assert_eq!(v.get_request_id(), 0xCDF0);
        assert_eq!(v.get_session_id(), 0xCDF0);
    }

    #[test]
    fn test_increments_overflow() {
        let mut v = PacketId::from_u64(0xFFFF_FFFF_FFFF_FFFF);
        v.increment_packet_id();
        assert_eq!(v.get_packet_id(), 0);
        assert_eq!(v.get_request_id(), 0xFFFF);
        assert_eq!(v.get_session_id(), 0xFFFF);

        v.increment_packet_id();
        v.increment_request_id();

        assert_eq!(v.get_packet_id(), 0);
        assert_eq!(v.get_request_id(), 0);
        assert_eq!(v.get_session_id(), 0);
    }
}
