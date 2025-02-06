#[cfg(test)]
mod test;

// given a path it returns only the filename
pub(crate) fn get_filename_from_path(s: &str) -> String {
    s.split('/').last().unwrap_or(s).to_string()
}

// given a html file, it looks for img tags and returns a vector of the files inside them
pub(crate) fn get_media_inside_html_file(file_str: &str) -> Vec<String> {
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

const REQ_ID_MASK: u64 = 0xFFFF;
const PACKET_ID_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

const _PACKET_ID_LEN: u8 = 48;
const REQ_ID_LEN: u8 = 16;

pub(crate) type SessionId = u64;
pub(crate) type RequestId = u16;

// represents a unique identifier for packets
// it uses 16 bits for a requestID and 48 for a packetID
// they can be merged inside a 64 bit integer for compability with WG sessionID inside packets
#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub(crate) struct PacketId {
    packet_id: SessionId, // only 48 LSB used
    req_id: RequestId,
}
impl PacketId {
    pub(crate) fn new() -> Self {
        Self {
            packet_id: 0,
            req_id: 0,
        }
    }
    // decomposes the input into sessionID (48 most significant bits) and requestID (the 16 bit left)
    pub(crate) fn from_u64(n: u64) -> Self {
        Self {
            packet_id: n >> REQ_ID_LEN,
            req_id: (n & REQ_ID_MASK) as u16,
        }
    }

    pub(crate) fn get_packet_id(&self) -> u64 {
        self.packet_id
    }

    pub(crate) fn get_request_id(&self) -> u16 {
        self.req_id
    }

    // returns a 64 bit the contains both packet and request IDs
    // used for quickly obtaining the WG session_id
    pub(crate) fn get_session_id(&self) -> u64 {
        (self.packet_id << REQ_ID_LEN) | u64::from(self.req_id)
    }

    pub(crate) fn increment_packet_id(&mut self) {
        self.packet_id += 1;
        self.packet_id &= PACKET_ID_MASK;
    }

    pub(crate) fn increment_request_id(&mut self) {
        self.packet_id = 0; // new request has to reset the session id
        self.req_id = self.req_id.wrapping_add(1);
    }
}
