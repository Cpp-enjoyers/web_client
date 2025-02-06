#[cfg(test)]
mod utils_test {
    use crate::utils::{get_filename_from_path, get_media_inside_html_file, PacketId};

    #[test]
    pub fn filename_from_path() {
        let path = "a/b/c/d/e/ciao.txt".to_string();
        assert_eq!(get_filename_from_path(&path), "ciao.txt".to_string());

        let path = "ciao.txt".to_string();
        assert_eq!(get_filename_from_path(&path), "ciao.txt".to_string());
    }

    #[test]
    fn file_parsing() {
        assert_eq!(
            get_media_inside_html_file(&"suhbefuiwfbwob".to_string()),
            Vec::<String>::new()
        );
        assert_eq!(
            get_media_inside_html_file(&"-----------<img src=\"youtube.com\"\\>".to_string()),
            vec!["youtube.com".to_string()]
        );
        assert_eq!(
            get_media_inside_html_file(
                &"-----------<img src=\"/usr/tmp/folder/subfolder/pic.jpg\"\\>".to_string()
            ),
            vec!["/usr/tmp/folder/subfolder/pic.jpg".to_string()]
        );
    }

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
