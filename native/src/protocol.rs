//! Ultimate 2 vendor HID framing. See docs/NATIVE_PROTOCOL.md.
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, String>;

pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff;
    for &b in bytes {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xa001 } else { 0 };
        }
    }
    crc
}

#[derive(Debug, Clone)]
pub struct Packet {
    pub class: u8,
    pub cmd: u16,
    pub sub: u16,
    pub total: u32,
    pub offset: u32,
    pub length: usize,
    pub data: Vec<u8>,
}
impl Packet {
    pub fn new(class: u8, cmd: u16) -> Self {
        Self {
            class,
            cmd,
            sub: 0,
            total: 0,
            offset: 0,
            length: 0,
            data: vec![],
        }
    }
    pub fn encode(&self) -> Result<[u8; 64]> {
        if self.length > 46 || self.data.len() > self.length {
            return Err("HID packet exceeds 46 bytes".into());
        }
        let mut b = [0; 64];
        b[0] = 0x81;
        b[1] = self.class;
        b[2..4].copy_from_slice(&self.cmd.to_le_bytes());
        b[4..6].copy_from_slice(&self.sub.to_le_bytes());
        b[6..8].copy_from_slice(&(self.length as u16).to_le_bytes());
        let crc = if self.data.is_empty() {
            0
        } else {
            crc16(&self.data)
        };
        b[8..10].copy_from_slice(&crc.to_le_bytes());
        b[10..14].copy_from_slice(&self.total.to_le_bytes());
        b[14..18].copy_from_slice(&self.offset.to_le_bytes());
        b[18..18 + self.data.len()].copy_from_slice(&self.data);
        Ok(b)
    }
}

pub fn reply(b: &[u8], p: &Packet) -> Option<Vec<u8>> {
    if b.len() < 18
        || b[0] != 2
        || b[1] != p.class
        || word(b, 2) != if p.class == 5 { 0 } else { 4 }
        || word(b, 4) != p.cmd
    {
        return None;
    }
    if p.class == 4 && [1, 2, 0x102, 0x103].contains(&p.cmd) && dword(b, 14) != p.offset {
        return None;
    }
    let n = word(b, 6) as usize;
    if n > 46 || b.len() < 18 + n {
        return None;
    }
    Some(b[18..18 + n].to_vec())
}
pub fn word(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn dword(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub trait Transport {
    fn write(&self, bytes: &[u8]) -> Result<()>;
    fn read(&self, timeout_ms: i32) -> Result<Vec<u8>>;
    fn request(&self, p: Packet, timeout: Duration) -> Result<Vec<u8>> {
        // Drop queued replies before reusing a command ID, especially live-state reads.
        for _ in 0..128 {
            if self.read(0)?.is_empty() {
                break;
            }
        }
        self.write(&p.encode()?)?;
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            let b = self.read(
                end.saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(100) as i32,
            )?;
            if let Some(data) = reply(&b, &p) {
                return Ok(data);
            }
        }
        Err(format!("Controller did not answer command {:04x}", p.cmd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_discard_queued_stale_replies() {
        use std::{cell::RefCell, collections::VecDeque};
        struct Queue(RefCell<VecDeque<Vec<u8>>>);
        fn response(value: u8) -> Vec<u8> {
            let mut b = vec![0; 19];
            b[..8].copy_from_slice(&[2, 4, 4, 0, 3, 0, 1, 0]);
            b[18] = value;
            b
        }
        impl Transport for Queue {
            fn write(&self, _: &[u8]) -> Result<()> {
                self.0.borrow_mut().push_back(response(200));
                Ok(())
            }
            fn read(&self, _: i32) -> Result<Vec<u8>> {
                Ok(self.0.borrow_mut().pop_front().unwrap_or_default())
            }
        }
        let q = Queue(RefCell::new(VecDeque::from([response(45), response(0)])));
        assert_eq!(
            q.request(Packet::new(4, 3), Duration::from_millis(100))
                .unwrap(),
            vec![200]
        );
    }
    #[test]
    fn captured_firmware_identity_response() {
        // Read-only capture from the real Ultimate 2, firmware 1.10.
        let bytes = [
            2, 5, 0, 0, 8, 0, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0x12, 0x60,
        ];
        assert_eq!(reply(&bytes, &Packet::new(5, 8)), Some(vec![0x12, 0x60]));
        assert_eq!(reply(&bytes, &Packet::new(5, 4)), None);
        let mut error = bytes;
        error[2] = 1;
        assert_eq!(reply(&error, &Packet::new(5, 8)), None);
    }
    #[test]
    fn frames_and_crc() {
        assert_eq!(crc16(b"123456789"), 0x4b37);
        let mut p = Packet::new(4, 0x103);
        p.sub = 0x302;
        p.offset = 4096;
        p.total = 32;
        p.length = 2;
        p.data = vec![1, 2];
        let b = p.encode().unwrap();
        assert_eq!(&b[..8], &[0x81, 4, 3, 1, 2, 3, 2, 0]);
        assert_eq!(dword(&b, 14), 4096);
        let mut r = b;
        r[0] = 2;
        r[2..4].copy_from_slice(&4u16.to_le_bytes());
        r[4..6].copy_from_slice(&p.cmd.to_le_bytes());
        assert_eq!(reply(&r, &p), Some(vec![1, 2]));
        r[6] = 255;
        assert_eq!(reply(&r, &p), None);
        p.length = 47;
        assert!(p.encode().is_err());
        p.length = 1;
        assert!(p.encode().is_err());
    }
}
