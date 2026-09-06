/// Decode output from the native utilities launched without a console window.
pub fn decode(bytes: &[u8]) -> String {
    #[cfg(target_os = "windows")]
    {
        // These processes do not inherit a console/code-page override. Use the
        // system OEM page, which can also be UTF-8 on modern Windows installs.
        decode_code_page(bytes, windows_sys::Win32::Globalization::CP_OEMCP)
    }
    #[cfg(not(target_os = "windows"))]
    {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(target_os = "windows")]
fn decode_code_page(bytes: &[u8], code_page: u32) -> String {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;

    if bytes.is_empty() {
        return String::new();
    }
    let Ok(byte_count) = i32::try_from(bytes.len()) else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    // The first call asks Windows for the required UTF-16 buffer size.
    unsafe {
        let count = MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            byte_count,
            std::ptr::null_mut(),
            0,
        );
        if count == 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; count as usize];
        let written = MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            byte_count,
            wide.as_mut_ptr(),
            count,
        );
        if written == 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        String::from_utf16_lossy(&wide[..written as usize])
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn decodes_cp936_and_utf8_without_corrupting_chinese_fields() {
        assert_eq!(
            decode_code_page(b"\xd0\xc5\xba\xc5 : 86%", 936),
            "信号 : 86%"
        );
        assert_eq!(decode_code_page("测试网络".as_bytes(), 65001), "测试网络");
        assert_eq!(decode_code_page(b"", 936), "");
    }

    #[test]
    fn decoded_output_reaches_the_real_wifi_parser() {
        let mut raw = b"SSID 1 : ".to_vec();
        raw.extend_from_slice(b"\xb2\xe2\xca\xd4\xcd\xf8\xc2\xe7\n");
        raw.extend_from_slice(b"    BSSID 1 : aa:bb:cc:dd:ee:ff\n    \xd0\xc5\xba\xc5 : 86%\n    \xd0\xc5\xb5\xc0 : 149\n");
        let decoded = decode_code_page(&raw, 936);
        let rows = crate::wifi::parse_windows_netsh(&decoded);
        assert_eq!(rows[0].ssid, "测试网络");
        assert_eq!(rows[0].signal_dbm, -57);
        assert_eq!(rows[0].channel, 149);
    }
}
